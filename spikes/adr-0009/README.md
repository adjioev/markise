# Spikes for ADR 0009 (diagrams) — throwaway, not workspace members

- `merman-spike/`: renders `samples/*.mmd` with `merman` 0.9.2 (SVG only, 7 diagram types, no
  ELK/math/PNG); `NOPIPE=1` skips the resvg-safe post-processing; `fuzz <files>` runs 3000
  random mutations under `catch_unwind`.
  `cargo build --release && NOPIPE=1 ./target/release/spike-merman ../samples/*.mmd`
- `dugong-spike/`: layout time of synthetic graphs (6…2000 nodes) with `dugong` 0.9.2 and
  `rust-sugiyama` 0.4, plus 2000 random compound graphs under `catch_unwind`.

Results (Apple M1 Max) are summarised in `docs/adr/0009-diagrams.md`. Generated `*.svg` files and
`target/` are not committed.
