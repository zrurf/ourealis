//! wgpu compute backend.
//!
//! Two kernels cover the parallel work:
//!
//! * `cost_gemm` — one invocation per `(cell, mode)` pair, looping over the
//!   feature dimension. `D` is small (8–16 in practice), so a tiled shared-memory
//!   product would add complexity without changing the arithmetic intensity;
//!   the weight column is kept in registers instead;
//! * `noise_batch` — one invocation per individual, looping over samples, because
//!   the Ornstein-Uhlenbeck recursion is sequential in time and only the batch
//!   dimension parallelises.
//!
//! Both kernels use the counter-based random source of [`super::counter_gaussian`],
//! reimplemented in WGSL with the same constants, so a batch generated on the GPU
//! is bit-identical to the CPU reference.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use crate::error::{CoreError, Result};

use super::{
    ComputeBackend, CostBatch, FeatureTensor, NoiseBatch, NoiseBatchSpec, ProjectionAnswer,
    ProjectionBatch, ProjectionBatchOut, WeightMatrix,
};

const COST_SHADER: &str = include_str!("shaders/cost_gemm.wgsl");
const NOISE_SHADER: &str = include_str!("shaders/noise_batch.wgsl");
const PROJECTION_SHADER: &str = include_str!("shaders/projection_check.wgsl");

/// Uniform block of the cost kernel.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CostParams {
    cells: u32,
    dims: u32,
    modes: u32,
    c0: f32,
}

/// Uniform block of the noise kernel.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct NoiseParams {
    individuals: u32,
    samples: u32,
    seed_lo: u32,
    seed_hi: u32,
    dt: f32,
    sigma: f32,
    tau: f32,
    mean: f32,
    white_sigma: f32,
    _padding: [u32; 3],
}

/// A device, queue and compiled pipelines, created once per simulation.
pub struct WgpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    cost_pipeline: wgpu::ComputePipeline,
    projection_pipeline: wgpu::ComputePipeline,
    cost_layout: wgpu::BindGroupLayout,
    projection_layout: wgpu::BindGroupLayout,
    noise_pipeline: wgpu::ComputePipeline,
    noise_layout: wgpu::BindGroupLayout,
}

impl WgpuBackend {
    /// Creates the backend on the best available adapter.
    ///
    /// Returns an error rather than panicking when no adapter or device is
    /// available; callers decide whether to fall back to the CPU.
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|error| CoreError::NoAdapter {
            detail: format!("{error:?}"),
        })?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ourealis"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            ..Default::default()
        }))
        .map_err(|error| CoreError::Backend {
            backend: "wgpu",
            message: format!("device creation failed: {error:?}"),
        })?;

        let cost_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cost_gemm"),
            source: wgpu::ShaderSource::Wgsl(COST_SHADER.into()),
        });
        let noise_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("noise_batch"),
            source: wgpu::ShaderSource::Wgsl(NOISE_SHADER.into()),
        });
        let projection_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("projection_check"),
            source: wgpu::ShaderSource::Wgsl(PROJECTION_SHADER.into()),
        });

        let storage_read = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_write = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let cost_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cost_gemm_layout"),
            entries: &[
                uniform(0),
                storage_read(1),
                storage_read(2),
                storage_write(3),
            ],
        });
        let noise_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("noise_batch_layout"),
            entries: &[uniform(0), storage_write(1), storage_write(2)],
        });
        let projection_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("projection_check_layout"),
            entries: &[
                uniform(0),
                storage_read(1),
                storage_read(2),
                storage_read(3),
                storage_write(4),
            ],
        });

        let make_pipeline = |label: &str,
                             module: &wgpu::ShaderModule,
                             layout: &wgpu::BindGroupLayout| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };

        let cost_pipeline = make_pipeline("cost_gemm", &cost_module, &cost_layout);
        let noise_pipeline = make_pipeline("noise_batch", &noise_module, &noise_layout);
        let projection_pipeline =
            make_pipeline("projection_check", &projection_module, &projection_layout);

        Ok(Self {
            device,
            queue,
            adapter_name: info.name,
            cost_pipeline,
            cost_layout,
            noise_pipeline,
            noise_layout,
            projection_pipeline,
            projection_layout,
        })
    }

    /// Name of the adapter in use.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    fn storage_buffer(&self, label: &str, values: &[f32]) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(values),
                usage: wgpu::BufferUsages::STORAGE,
            })
    }

    fn readback(&self, buffer: &wgpu::Buffer, len: usize) -> Result<Vec<f32>> {
        let size = (len * std::mem::size_of::<f32>()) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);

        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| CoreError::Backend {
                backend: "wgpu",
                message: format!("device poll failed during readback: {error}"),
            })?;
        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(CoreError::Backend {
                    backend: "wgpu",
                    message: format!("buffer mapping failed: {error}"),
                });
            }
            Err(error) => {
                return Err(CoreError::Backend {
                    backend: "wgpu",
                    message: format!("buffer mapping callback was dropped: {error}"),
                });
            }
        }
        // Returning zeros instead of failing would be worse than an error: a
        // caller asking whether ground is passable would be told yes everywhere,
        // and the offset cap would place the runner through obstacles with
        // nothing surfacing anywhere.
        let view = slice
            .get_mapped_range()
            .map_err(|error| CoreError::Backend {
                backend: "wgpu",
                message: format!("mapped buffer view failed: {error}"),
            })?;
        let out: Vec<f32> = bytemuck::cast_slice(&view).to_vec();
        drop(view);
        staging.unmap();
        Ok(out)
    }
}

impl WgpuBackend {
    /// Creates a storage buffer from `u32` values.
    fn storage_buffer_u32(&self, label: &str, values: &[u32]) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE,
            })
    }
}

impl ComputeBackend for WgpuBackend {
    fn name(&self) -> String {
        format!("wgpu:{}", self.adapter_name)
    }

    fn cost_field_batch(
        &self,
        features: &FeatureTensor,
        weights: &WeightMatrix,
        c0: f32,
    ) -> Result<CostBatch> {
        if features.dims != weights.dims {
            return Err(CoreError::DimensionMismatch {
                weights: weights.dims,
                features: features.dims,
            });
        }
        let params = CostParams {
            cells: features.cells as u32,
            dims: features.dims as u32,
            modes: weights.modes as u32,
            c0,
        };
        let params_buffer = self
            .device
            .create_buffer_init_checked("cost_params", bytemuck::bytes_of(&params));
        let feature_buffer = self.storage_buffer("features", &features.values);
        let weight_buffer = self.storage_buffer("weights", &weights.values);

        let output_len = features.cells * weights.modes;
        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cost_output"),
            size: (output_len * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cost_gemm"),
            layout: &self.cost_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: feature_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: weight_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.cost_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let workgroups = (output_len as u32).div_ceil(64);
            pass.dispatch_workgroups(workgroups.max(1), 1, 1);
        }
        self.queue.submit([encoder.finish()]);

        Ok(CostBatch {
            cells: features.cells,
            modes: weights.modes,
            values: self.readback(&output_buffer, output_len)?,
        })
    }

    fn projection_check_batch(&self, batch: &ProjectionBatch) -> Result<ProjectionBatchOut> {
        batch.validate()?;
        if batch.points == 0 {
            return Ok(ProjectionBatchOut::default());
        }
        #[repr(C)]
        #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
        struct Params {
            points: u32,
            width: u32,
            height: u32,
            resolution: f32,
            origin_x: f32,
            origin_y: f32,
            padding0: u32,
            padding1: u32,
        }
        let params = Params {
            points: batch.points as u32,
            width: batch.grid_dims[0],
            height: batch.grid_dims[1],
            resolution: batch.resolution.max(1e-9),
            origin_x: batch.grid_origin[0],
            origin_y: batch.grid_origin[1],
            padding0: 0,
            padding1: 0,
        };
        use wgpu::util::DeviceExt;
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("projection_params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let xy_buffer = self.storage_buffer("projection_xy", &batch.xy);
        let forbidden_buffer = self.storage_buffer_u32("projection_forbidden", &batch.forbidden);
        let distance_buffer = self.storage_buffer("projection_distance", &batch.distance);
        // Two f32 per point in one buffer: the answer and its flags. A compute
        // stage is only guaranteed four storage bindings, and the inputs already
        // take four.
        let out_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("projection_out"),
            size: (batch.points * 8) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projection_check"),
            layout: &self.projection_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: xy_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: forbidden_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: distance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: out_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("projection_check"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.projection_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups((batch.points as u32).div_ceil(64), 1, 1);
        }
        self.queue.submit([encoder.finish()]);

        let values = self.readback(&out_buffer, batch.points * 2)?;
        let answers = (0..batch.points)
            .map(|index| {
                let flags = values[index * 2 + 1] as u32;
                ProjectionAnswer {
                    forbidden: flags & 1 == 1,
                    distance_m: values[index * 2],
                    outside: flags & 2 == 2,
                }
            })
            .collect();
        Ok(ProjectionBatchOut { answers })
    }

    fn noise_batch(&self, spec: &NoiseBatchSpec) -> Result<NoiseBatch> {
        spec.validate()?;
        let params = NoiseParams {
            individuals: spec.individuals as u32,
            samples: spec.samples as u32,
            seed_lo: spec.seed as u32,
            seed_hi: (spec.seed >> 32) as u32,
            dt: spec.dt,
            sigma: spec.drift.sigma,
            tau: spec.drift.tau,
            mean: spec.drift.mean,
            white_sigma: spec.white_sigma,
            _padding: [0; 3],
        };
        let params_buffer = self
            .device
            .create_buffer_init_checked("noise_params", bytemuck::bytes_of(&params));

        let len = spec.individuals * spec.samples;
        let byte_len = (len * std::mem::size_of::<f32>()) as u64;
        let drift_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noise_drift"),
            size: byte_len,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let white_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noise_white"),
            size: byte_len,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("noise_batch"),
            layout: &self.noise_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: drift_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: white_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.noise_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let workgroups = (spec.individuals as u32).div_ceil(64);
            pass.dispatch_workgroups(workgroups.max(1), 1, 1);
        }
        self.queue.submit([encoder.finish()]);

        Ok(NoiseBatch {
            individuals: spec.individuals,
            samples: spec.samples,
            drift: self.readback(&drift_buffer, len)?,
            white: self.readback(&white_buffer, len)?,
        })
    }
}

/// Helper trait so the backend can create uniform buffers through `wgpu::util`.
trait CreateBufferInitChecked {
    fn create_buffer_init_checked(&self, label: &str, contents: &[u8]) -> Arc<wgpu::Buffer>;
}

impl CreateBufferInitChecked for wgpu::Device {
    fn create_buffer_init_checked(&self, label: &str, contents: &[u8]) -> Arc<wgpu::Buffer> {
        use wgpu::util::DeviceExt;
        Arc::new(self.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::UNIFORM,
        }))
    }
}
