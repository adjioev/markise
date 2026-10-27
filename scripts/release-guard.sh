#!/usr/bin/env bash
# Fail if a GitHub release for <tag> exists and is already published (not a draft).
# The release workflow only creates new drafts; publishing is a manual step.
#
# usage: GH_TOKEN=… scripts/release-guard.sh <tag> [owner/repo]
set -euo pipefail
tag="${1:?tag}"
repo="${2:-${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY or pass owner/repo}}"

# "Get a release by tag name" only returns published releases (drafts have no public tag
# binding), so a hit here means the tag is already published.
if out="$(gh api "repos/$repo/releases/tags/$tag" --jq '.draft' 2>&1)"; then
  if [[ "$out" == "false" ]]; then
    echo "::error::release $tag is already published; refusing to modify it (delete or re-tag manually)" >&2
    exit 1
  fi
elif [[ "$out" != *"Not Found"* && "$out" != *"404"* ]]; then
  echo "::error::could not query release $tag: $out" >&2
  exit 1
fi

# Belt and braces: also scan the release list (drafts appear only here).
published="$(gh api "repos/$repo/releases?per_page=100" \
  --jq "[.[] | select(.tag_name == \"$tag\" and (.draft | not))] | length")"
if [[ "$published" != 0 ]]; then
  echo "::error::release $tag is already published; refusing to modify it" >&2
  exit 1
fi
echo "no published release for $tag: a new draft may be created"
