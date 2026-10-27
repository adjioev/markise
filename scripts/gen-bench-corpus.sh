#!/bin/sh
# Generates the benchmark corpus (1 MB, 10 MB, tables, math, CJK/CRLF) into benches/corpus/.
# The files are deterministic and gitignored; benchmarks regenerate them in memory if absent.
set -eu
cd "$(dirname "$0")/.."
cargo run --release -q -p markise-corpus -- "${1:-benches/corpus}"
