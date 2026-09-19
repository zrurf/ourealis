// Batched feasibility and clearance queries.
//
// One invocation per query point. The point is converted to a cell the same way
// the Rust reference does — floor of the offset over the resolution — then the
// bitmap bit and the distance value are read. The arithmetic is identical on both
// sides on purpose: the parity test compares answers, so any "clever" shortcut here
// would make it compare two different questions.

struct Params {
    points: u32,
    width: u32,
    height: u32,
    resolution: f32,
    origin_x: f32,
    origin_y: f32,
    padding0: u32,
    padding1: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> xy: array<f32>;
@group(0) @binding(2) var<storage, read> forbidden: array<u32>;
@group(0) @binding(3) var<storage, read> distance: array<f32>;
// Two values per point: the distance first, then the flags. Both are 32-bit and
// the flag values are small integers, so packing them into one buffer is exact —
// and it keeps the kernel inside the four storage bindings a compute stage is
// guaranteed, which the five it would otherwise need are not.
@group(0) @binding(4) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= params.points) {
        return;
    }

    let x = xy[index * 2u];
    let y = xy[index * 2u + 1u];
    let cell_x = floor((x - params.origin_x) / params.resolution);
    let cell_y = floor((y - params.origin_y) / params.resolution);

    if (cell_x < 0.0 || cell_y < 0.0
        || u32(cell_x) >= params.width || u32(cell_y) >= params.height) {
        out[index * 2u] = 0.0;
        out[index * 2u + 1u] = 3.0;
        return;
    }

    let cx = u32(cell_x);
    let cy = u32(cell_y);
    let cell = cy * params.width + cx;
    let word = forbidden[cell / 32u];
    let bit = (word >> (cell % 32u)) & 1u;
    out[index * 2u] = distance[cell];
    out[index * 2u + 1u] = f32(bit);
}
