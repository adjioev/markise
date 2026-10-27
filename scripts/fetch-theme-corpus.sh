#!/usr/bin/env bash
# Download the community theme corpus (test input for the CSS importer, spec 03 §2) into the
# gitignored `.theme-cache/`. Each repository is fetched once per pinned commit as a tarball.
# The themes are never committed (AGENTS.md clean-room rules); only the manifest is.
#
# usage: scripts/fetch-theme-corpus.sh [--force]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/crates/markise-theme/tests/theme-corpus.tsv"
cache="${MARKISE_THEME_CACHE:-$root/.theme-cache}"
force=0
[[ "${1:-}" == "--force" ]] && force=1

mkdir -p "$cache"
fetched=0
failed=0
while IFS=$'\t' read -r id repo commit css license; do
  [[ -z "$id" || "$id" == \#* ]] && continue
  dir="$cache/${repo//\//__}-${commit:0:12}"
  if [[ -d "$dir" && $force -eq 0 ]]; then
    continue
  fi
  echo "fetching $repo@${commit:0:12} ($id, license: $license)"
  tmp="$(mktemp -d)"
  if curl -fsSL --retry 2 "https://codeload.github.com/$repo/tar.gz/$commit" -o "$tmp/src.tar.gz" \
      && mkdir -p "$tmp/x" && tar -xzf "$tmp/src.tar.gz" -C "$tmp/x" --strip-components=1; then
    rm -rf "$dir"
    mv "$tmp/x" "$dir"
    fetched=$((fetched + 1))
  else
    echo "  failed: $repo@$commit" >&2
    failed=$((failed + 1))
  fi
  rm -rf "$tmp"
done < "$manifest"

missing=0
while IFS=$'\t' read -r id repo commit css license; do
  [[ -z "$id" || "$id" == \#* ]] && continue
  f="$cache/${repo//\//__}-${commit:0:12}/$css"
  if [[ ! -f "$f" ]]; then
    echo "missing theme file for $id: $f" >&2
    missing=$((missing + 1))
  fi
done < "$manifest"

echo "theme corpus in $cache: $fetched repositories fetched, $failed failed, $missing theme files missing"
[[ $failed -eq 0 && $missing -eq 0 ]]
