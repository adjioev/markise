# Performance measurements

Product budgets remain NFR-1…10 in the specification: startup <250 ms, 10 MB open <1 s,
keystroke p99 <16 ms, idle CPU ~0%. Measure optimized builds on a specified machine and report
build time separately. Do not turn these wall-clock targets into debug test assertions.

Existing entry points cover the work moved out of default tests (#98):

| Scenario | Measurement |
| --- | --- |
| Adversarial parser/inline scaling | `cargo bench -p markise-md --bench parse -- md-adversarial` |
| Wikilink close-search speed | `cargo bench -p markise-md --bench parse -- md-wikilinks-adversarial` |
| Diagram rendering speed | `cargo bench -p markise-blocks --bench diagram` |
| TOC export expansion | `cargo bench -p markise-export` |
| Large-document edit latency | `cargo bench -p markise-engine --bench typing` |
| `:::` blocks on large documents | `cargo bench -p markise-engine --bench directives` |
| Native block views on large documents | `cargo bench -p markise-engine --bench native_blocks` |
| Large-document open/layout | `cargo bench -p markise-layout --bench layout` |
| Dragging a section in the outline | `cargo bench -p markise-ui-egui --bench section_panel` |
| 100 MiB scaling | `cargo bench -p markise-ui-egui --bench huge` (see its environment options) |

`cargo xtask heavy` retains stress/correctness checks; benchmarks characterize speed. A warm
fast-suite target of a few minutes is a CI feedback target, separate from product NFR budgets.
