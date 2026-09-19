# 使用文档

## 工作区

| Crate | 内容 |
|---|---|
| `ourealis-map-format` | OMF 静态地图容器：读取器、写入器、构建器、编码器、指纹、补丁。 |
| `ourealis-core` | 模拟器：环境、规划、运动学、传感器、评价、计算后端。 |

依赖单向：`ourealis-core` 通过 `ourealis-map-format` 读地图，反向不存在依赖。

## 构建与验收

```bash
cargo build --workspace
cargo test  --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo doc   --workspace --no-deps
```

GPU 后端由默认开启的 `gpu` feature 控制。使用 `--no-default-features` 可构建纯 CPU 版本：CPU
后端是每个内核的完整实现，也是 GPU 所对照的语义参考。

## 示例

```bash
# 合成校园地图 -> OMF 镜像 -> 路径 -> 轨迹 -> 传感器 -> 落盘
cargo run -p ourealis-core --example campus_run --release

# 一条路线上的群体模拟，含批量统计
cargo run -p ourealis-core --example population --release

# 把下载的录音转换为测试读取的表（见下文"真实数据"）
cargo run -p ourealis-core --example fetch_real_data

# 用录音标定步态参数；--write 会更新参考表
cargo run -p ourealis-core --release --example calibrate -- --verbose
cargo run -p ourealis-core --release --example calibrate -- --write

# 只生成并写出一张合成地图，不做任何模拟
cargo run -p ourealis-map-format --example build_synthetic_map
```

`campus_run` 在临时目录下写出 `run.json`、`csv/*.csv` 与 `track.geojson` 并打印该目录路径；
`population` 打印每个个体的汇总。两者都不需要地图文件：合成地图在内存中生成。

## 代码中的一次运行

```rust
use glam::DVec2;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::sim::{MapSource, Simulator};

let output = Simulator::builder()
    .map(MapSource::omf("campus.omf"))          // 或 ::synthetic(spec) / ::bytes(image)
    .person(PersonParams::preset(Preset::Moderate))
    .standard(StandardRequest::new(
        DVec2::new(10.0, 10.0),
        DVec2::new(300.0, 200.0),
    ))
    .seed(42)
    .individual(0)
    .build()?
    .run()?;

println!("{:.1} s, {} GNSS fixes", output.duration_s(), output.sensors.gnss.len());
```

`build()` 解析地图、成本权重、计算后端与规划模式，得到 `Simulator`；`run()` 产出
`SimulationOutput`：

| 字段 | 内容 |
|---|---|
| `truth` | `Vec<TruthState>`：位置、速度、加速度、姿态、高度、有效曲率、坡度，以及静立与转身标记。 |
| `sensors` | `Sensors { gnss, gnss_gaps, imu { accel, gyro }, mag, baro, mount, .. }`。 |
| `trajectory` | `Trajectory`：采样序列、带链接通道的路径、速度剖面、弹跳配置、偏移、机动与圈数。 |
| `route` | `RouteSummary`：路径点、长度、成本、每段候选与选择概率。 |
| `metrics` | `Option<MetricsReport>`；`with_metrics` 打开时存在（默认打开）。 |
| `manifest` | 可复现性凭据：地图、模式、个体参数、种子、个体号、实际解析到的后端与采样率。 |

## 规划模式

```rust
use ourealis_core::plan::{Checkpoint, LoopRequest, StandardRequest, ViaSemantics, Waypoint};

// 起点 -> 若干有序途经点 -> 终点。每个途经点携带行为语义：
//   Pass        匀速通过（默认）
//   Slow        进入其半径内降速至局部上限的约 65%
//   Dwell{..}   到点后静止给定时间
let request = StandardRequest::new(start, goal)
    .via(Waypoint::new(mid).with_semantics(ViaSemantics::Dwell { duration_s: 15.0 }));

// 闭环路线，重复给定圈数
let request = LoopRequest::new(start, 3);

// 运行中改道
let request = StandardRequest::new(start, goal);
let checkpoints = [Checkpoint { position: elsewhere, issued_at_s: 120.0 }];
```

`run()` 执行已规划路线并忽略打卡点列表；`run_dynamic()` 按时间顺序消费打卡点，每次都用跑者当时的
状态重新规划，并在配置的切换窗口内把新轨迹混合进来。

## 配置

`SimulationConfig` 是完整的配置树。`SimulationConfig::default()` 表示一台消费级手机且打开指标；
另有两个供测试使用的构造器：

```rust
use ourealis_core::sensor::SensorConfig;
use ourealis_core::sim::SimulationConfig;

let mut config = SimulationConfig::default();   // 真实噪声，打开指标
config.sensors = SensorConfig::clean();         // 去掉传感器噪声与事件
config.sensors = SensorConfig::calibrated();    // 真实噪声，确定性事件
```

子配置按阶段分组：`cost`（成本模型）、`coarse`（粗格块）、`prm`（路标图）、
`route`/`loop_route`/`dynamic`（规划）、`motion`（速度上限、剖面、偏移、姿态、机动）、`sensors`
与 `backend`。

`SensorConfig::force_deterministic_events` 会覆盖地图为区域事件声明的触发模式。标定与回归运行
需要它：若使用独立抽样，相同输入的两次运行会产生不同的传感器数据，优化器会把事件随机性当成目标
函数的一部分去追。

采样率可配置：IMU 默认 100 Hz（与真值同频）、GNSS 1 Hz、磁力计 50 Hz、气压计 25 Hz。时间戳为
`t0 + k / f`，严格单调递增。

## 群体批量模拟

```rust
use ourealis_core::person::{PersonSampler, Preset};
use ourealis_core::sim::BatchRunner;

let people = PersonSampler::preset(Preset::Moderate).sample_population(seed, 24)?;
let batch = BatchRunner::new(simulator);
let outputs = batch.run(&people)?;                       // rayon 并行

let frequencies = batch.choice_frequencies(&outputs);    // 路径选择频率直方图
let fit = batch.cadence_speed_fit(&outputs);             // 步频-速度回归
```

个体之间完全独立，随机流按键中的个体号派生，因此并行批量产生的输出与串行完全一致。`BatchRunner`
只加载一次环境，供所有个体复用。

## 导出

```rust
use ourealis_core::sim::export;

export::write_json(&output, "run.json")?;         // 整个输出，serde JSON
export::write_csv_dir(&output, "csv/")?;          // truth、gnss、accel、gyro、mag、baro
export::write_geojson(&output, frame, "track.geojson")?;
```

`write_csv_dir` 每条数据流写一张表。`truth.csv` 含时间、位置、高度及其地形分量与弹跳分量、速度、
航向、俯仰、横滚、有效曲率、横向偏移、坡度与静立/转身标记——正是各传感器所依据的那些信号，下游
算法可以直接拿它与派生它的输入对照。地图带参考点时 `gnss.csv` 的经纬度写小数度，位置在任何情况下
都同时给出局部米制坐标。

## 真实数据

标定与对照层使用的录音不进入版本控制。把下列任意归档放进 `data/raw/` 后转换：

| 归档名 | 数据集 |
|---|---|
| `wisdm2.zip` | WISDM 2.0 手机与手表数据（UCI 507） |
| `motionsense.zip` | MotionSense |
| `dasa.zip` | Daily and Sports Activities（UCI 256） |
| `har.zip` | Human Activity Recognition（UCI 240） |

```bash
cargo run -p ourealis-core --example fetch_real_data
```

工具在 `data/real/` 下按通道各写一张表，并附 `.meta` 边车文件记录采样率与来源，同时打印它没找到的
归档地址。同时含加速度计与陀螺仪的数据集会产出两张表。`cargo test` 会读取目录中现有的全部表，因此
对照层随数据集增加而变宽；没有任何表时打印跳过原因并通过。

`data/real/targets.toml` 保存标定拟合出的参考值，由
`cargo run -p ourealis-core --example calibrate -- --write` 写入。仓库自带的 `PersonParams` 预设把
它们复现到 0.05 个容差以内。

## 接下来读什么

* 各部分如何拼起来：[architecture.md](architecture.md)
* 数值为什么是这样：[design.md](design.md)
* 地图容器：[map-format.md](map-format.md)
* 术语：[glossary.md](glossary.md)
* 测试保证了什么：[testing.md](testing.md)
