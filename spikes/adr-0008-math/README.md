# Spike for ADR 0008 (math rendering) — throwaway, not a workspace member

- `cmp`: RaTeX vs latex-rust on `cmp/src/corpus.txt` (coverage, median parse+layout time).
  `cargo run --release -p cmp`
- `size-base`, `size-ratex`, `size-latexrust`: release-binary size probes.
  `cargo build --release && stat -f '%z %N' target/release/size-*` (macOS)

Results are recorded in `docs/adr/0008-math-rendering.md`.
