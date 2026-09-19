# Ourealis 文档

Ourealis 在静态地图上模拟人类跑步轨迹，以及由同一条轨迹派生出的传感器数据流。给定一张地图、一个
起点与一个终点，系统输出一条运动学可行、统计特性接近真实人类跑步的轨迹，以及同一次跑步的 GNSS、
加速度计、陀螺仪、磁力计与气压计记录。

输出是用于测试下游算法的真值数据：行人航位推算、活动识别、路径选择建模，以及任何需要"误差、
步频与运动都完全已知"的数据的方法。

[English documentation](../en/index.md) · [术语表](glossary.md)

| 文档 | 内容 |
|---|---|
| [usage.md](usage.md) | 构建、运行、公开 API、导出与真实数据对照工具。 |
| [architecture.md](architecture.md) | crate 划分、模块地图、数据流、计算后端、可复现性。 |
| [design.md](design.md) | 算法设计：成本场、搜索、运动学、噪声、传感器、评价指标。 |
| [map-format.md](map-format.md) | OMF：字节布局、元数据、空间索引、编码器、指纹、补丁。 |
| [glossary.md](glossary.md) | 本项目定义和使用的术语。 |
| [testing.md](testing.md) | 测试分层，以及每一层分别锁住什么。 |

## 全文约定

* **单位**：长度用米，时间用秒，代码内部角度一律用弧度，只有字段名带 `_deg` 的配置项例外。
* **成本的两种量纲**：`*_cost_per_m` 表示每米路径的阻力，`*_equiv_m` 表示累计的**等效米**。等效米
  是路径选择的标定量纲，含义见[术语表](glossary.md)中的"每米成本与等效米"。
* **符号引用**：`crates/core/src/<模块>/<文件>.rs` 指本仓库中的路径，`Type::method` 与
  `module::function` 指具体实现。
* **数学记号**：行内公式写作 `$...$`，独立公式写作 `$$...$$`。坡度的符号约定统一为
  $i_\parallel = \nabla h \cdot \hat{v}$，上坡为正。
