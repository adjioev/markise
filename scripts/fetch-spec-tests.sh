#!/bin/sh
# Downloads the CommonMark 0.31.2 and GFM spec examples (public standards, CC-BY-SA, not
# committed) into .spec-cache/ for crates/markise-export/tests/conformance.rs.
set -eu
cd "$(dirname "$0")/.."
mkdir -p .spec-cache
curl -sSfL -o .spec-cache/commonmark-0.31.2.json https://spec.commonmark.org/0.31.2/spec.json
curl -sSfL -o .spec-cache/gfm-spec.txt https://raw.githubusercontent.com/github/cmark-gfm/master/test/spec.txt
echo "spec tests cached in .spec-cache/"
