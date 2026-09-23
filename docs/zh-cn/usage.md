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

## 服务与 Web 界面

服务是 `crates/service` 里的一个二进制（crate 名与二进制名均为 `ourealis`）：读一份 TOML、
提供三个门面、并把页面内嵌在二进制中。

```bash
pnpm --dir web install --frozen-lockfile         # 首次
cargo build --release -p ourealis                 # build.rs 调 Vite 构建并把 web/dist 内嵌
./target/release/ourealis --config dev.toml       # 不带参数则用默认值，且只监听回环
./target/release/ourealis --print-config          # 合并默认值后的有效配置
```

| 门面 | 开关 | 承载 |
|---|---|---|
| RPC | `server.rpc_enabled` | gRPC，包名 `ourealis.api.v1` |
| HTTP | `server.http_enabled` | `/api/v1` 下的 REST，外加 WebSocket 与 SSE |
| Web | `server.web_enabled` | 内嵌页面，挂在 `/`（要求 HTTP 门面开启） |

### 运行工作区

打开 `/` 即是界面。一次运行在 `/run` 一个页面里分五个阶段完成：

| 阶段 | 决定什么 |
|---|---|
| 地图 | 针对哪张地图规划 |
| 路线 | 模式与点位；在地图上单击与拖动即可绘制 |
| 跑者 | 预设、随机种子；专家模式下还有全部个体参数 |
| 传感器 | 采样率；专家模式下还有噪声与事件开关 |
| 运行 | 名称、指标开关，以及提交按钮 |

路线是**画出来的而不是填出来的**：在地图上单击放置起点与终点，拖动标记即可移动，双击可删除。
规划是自动的——路线一旦完整就请求预览——因此摘要（长度、路径比、预计用时）与候选表随路线变化即时更新，
不必先跑一遍才知道改动带来了什么。规划器自己的选择会在候选表里标出：运行走的就是那一条，
因为路径选择由路线与随机种子决定，而不是由表里选中哪一行决定。

**简单**只显示会改变结果的决策；**专家**全部显示并按语义分组，每组旁边的徽标显示与配方（或默认值）相差几项。
四张配方卡——校园慢跑、场地间歇、手机 + 手表、纯净真值——一次点击填满整份配置，之后仍可继续修改。

其余页面：`/maps`（导入、生成、下载）、`/maps/{id}`（预览，含区块检视与图层铺贴）、
`/maps/{id}/studio`（绘制区域与连接器并导出）、`/batch`（多样本批量）、`/omf`（结构树、元数据编辑、补丁）、
`/simulations/{id}` 及其 `/trajectory`、`/sensors`、`/audit` 三个分页、`/settings`。

### 任务：所有耗时操作都是 ticket

规划一条路线、取一次剖面、生成一张地图，耗时从几秒到几分钟，因此没有任何端点会为它挂住一个请求。
`POST /api/v1/tasks` 返回 `202` 与一个 ticket，工作在阻塞工作线程上跑：

```jsonc
// POST /api/v1/tasks
{"kind": "route_preview", "request": { /* 与 POST /simulations 相同的请求体 */ }}
{"kind": "route_plan",    "request": { /* … */ }}
{"kind": "synthetic_map", "spec": { "preset": "compact", "seed": 7 }, "name": "fixture"}
```

| 调用 | 回答 |
|---|---|
| `GET /api/v1/tasks/{id}` | `kind`、`state`（`queued`/`running`/`succeeded`/`failed`/`cancelled`）、`stage`、`elapsed_s`、`error` |
| `GET /api/v1/tasks/{id}/result` | 结果，按 kind 标记：`{"route": {…}}` 或 `{"map": {…}}`；运行中为 `409` |
| `GET /api/v1/tasks/{id}/result/ref` | 结果所在路径，不取回内容 |
| `DELETE /api/v1/tasks/{id}` | 取消：排队中立即停止，运行中在下一个阶段边界生效 |
| `GET /api/v1/tasks/{id}/events` | SSE：状态、阶段、日志，以及一个终态事件 |
| `GET /api/v1/tasks/{id}/ws` | 同一会话的 WebSocket 版本 |
| `GET /api/v1/tasks?kind=route_plan` | ticket 列表（最新在前，可按 kind 过滤） |

**运行本身也是任务**，`GET /tasks/{id}` 对运行 id 同样有效，所以一个轮询器可以管全部。
运行的数据仍在它自己的端点上（`/simulations/{id}/summary`、`/truth`、`/sensors`、`/export`），
因为一次完整运行装不进一个响应——`/tasks/{id}/result` 对运行返回 `415` 并说明原因。

不依赖地图的校验在提交时完成，所以"个体参数非法""采样率荒谬"是调用当场返回的 `400`，而不是要轮询才知道的失败。
需要地图的那些失败（镜像损坏、无可行路径）才表现为任务失败，并带服务端消息。

页面用的是同一套接口：需要等待的任务显示为模态环形加载器（带实测耗时与取消按钮），
其余在顶栏的任务托盘里报告。两者都不显示百分比——模拟器的一次运行是一次调用，没有可报的分数。

### 哪些位置可以放点

"跑步者能不能站在这里"由地图决定，而页面读不到：硬禁行是一层位图数据，页面只画地表。
`POST /api/v1/maps/{id}/feasibility` 回答一组点的可用性：

```jsonc
{"points": [{"x": 234, "y": 156}, {"x": 36, "y": 100}], "safe_radius_m": 0.75}
// { "items": [
//   {"point": …, "legal": false, "reason": "forbidden", "distance_m": 0,    "cell": [117, 78], "elevation_m": 13.8},
//   {"point": …, "legal": true,  "reason": "ok",        "distance_m": 0.75, "cell": [18, 50],  "elevation_m": 12.6}
// ]}
```

`reason` 取值 `ok`、`outside`（超出地图）、`forbidden`（禁行格）、`too_close`（可通行但与最近障碍的距离小于
`safe_radius_m`——正是模拟器偏移阶段自己的判定规则，因此这里通过的点规划器也会接受）。该端点只读位图与邻近格子，
不合成成本场、不建图，因此毫秒级返回；工作区在落点的那一刻就调用它，被拒的点用手柄的错误色标出，并用一句话说明原因。

### 前端验收

```bash
pnpm --dir web run typecheck      # vue-tsc 覆盖 src *与* tests，另有 node 侧配置
pnpm --dir web run lint           # oxlint
pnpm --dir web run format:check   # oxfmt
pnpm --dir web run build          # build.rs 内嵌的页面产物
pnpm --dir web run test:unit      # 纯逻辑，不启浏览器
pnpm --dir web run test:e2e       # 真实浏览器对着真实服务（会以 release 构建服务）
pnpm --dir web run test:fuzz      # 固定种子的随机输入
pnpm --dir web run test:monkey    # 固定种子的随机操作序列
```

e2e 泳道需要一个服务：`test:e2e` 会以 **release** 构建它，并按 `OUREALIS_SERVICE_URL`（默认
`http://127.0.0.1:8080`）去找。找不到服务时它**失败**而不是跳过，除非设置 `OUREALIS_ALLOW_SKIP=1`——
一个在缺少被验证栈时静默通过的泳道，比一个报出问题的泳道更糟。

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
