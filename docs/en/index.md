# Ourealis documentation

Ourealis simulates human running trajectories and the sensor streams derived from
them, on static maps. This directory is the reference documentation for the
project: what the system does, how it is built, and which conventions its
formats and data follow.

| Document | Content |
|---|---|
| [usage.md](usage.md) | Building, running, the public API, exports and the real-data harness. |
| [architecture.md](architecture.md) | Crates, module map, data flow, compute backends, reproducibility. |
| [design.md](design.md) | The algorithms: cost field, search, motion, noise, sensors, metrics. |
| [map-format.md](map-format.md) | OMF: byte layout, metadata, spatial index, codecs, fingerprints, patches. |
| [glossary.md](glossary.md) | The vocabulary this project defines and uses. |
| [testing.md](testing.md) | The test layers and what each one pins down. |

A Chinese translation of the same set lives in [`../zh-cn/`](../zh-cn/index.md).

## Conventions

* **Units.** Lengths are metres, times seconds, and angles radians inside the
  code; a configuration field in degrees is named with a `_deg` suffix.
* **Two cost scales.** `*_cost_per_m` is resistance per metre of path, and
  `*_equiv_m` is accumulated **equivalent metres** — the unit in which route
  choice is calibrated. See the [glossary](glossary.md).
* **References.** `crates/core/src/<module>/<file>.rs` refers to a path in this
  repository; `Type::method` and `module::function` refer to specific items.
* **Mathematics.** Inline formulas are written `$...$` and display formulas
  `$$...$$`. Grades follow one sign convention everywhere:
  $i_\parallel = \nabla h \cdot \hat{v}$, positive uphill.
