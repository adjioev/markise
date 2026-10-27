#!/usr/bin/env bash
# Validate a release tag as v<SemVer 2.0.0> and print its numeric core.
#
# usage: scripts/check-tag.sh <tag>
# prints: version=<major.minor.patch>   (prerelease and +build metadata stripped)
#
# Strict SemVer: numeric identifiers have no leading zeros (also numeric prerelease
# identifiers), no empty identifiers. Build metadata (+...) is allowed; it is ignored for
# version precedence and stripped from the macOS bundle version (scripts/package.sh derives
# the bundle's build number from the numeric core, so it is the same for every run of a tag).
set -euo pipefail
tag="${1:?tag}"
num='(0|[1-9][0-9]*)'
pre_id='(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
build_id='[0-9A-Za-z-]+'
re="^v${num}\.${num}\.${num}(-${pre_id}(\.${pre_id})*)?(\+${build_id}(\.${build_id})*)?$"
if ! [[ "$tag" =~ $re ]]; then
  echo "'$tag' is not v<major>.<minor>.<patch>[-prerelease][+build] (SemVer 2.0.0)" >&2
  exit 1
fi
major="${BASH_REMATCH[1]}" minor="${BASH_REMATCH[2]}" patch="${BASH_REMATCH[3]}"
echo "version=$major.$minor.$patch"
