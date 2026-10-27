#!/usr/bin/env bash
# Create a BRAND-NEW draft GitHub release for <tag> at <sha> and upload <files> to it.
#
# usage: GH_TOKEN=… [PRERELEASE=true] scripts/release-draft.sh <tag> <sha> <notes-file> <files…>
#
# PRERELEASE=true marks the draft as a GitHub pre-release (tags such as v0.1.0-beta.1).
#
# Never touches an existing release: the new release is identified only by the id returned
# from its creation (no lookup by tag), assets are only ever added to that release (no delete,
# no replace), and the run fails early if a published release for the tag exists
# (scripts/release-guard.sh). Older drafts for the same tag are left alone; delete stale
# drafts by hand. Server-side protection for published assets: enable "immutable releases"
# in the repository settings.
set -euo pipefail
tag="${1:?tag}"
sha="${2:?sha}"
notes="${3:?notes-file}"
shift 3
[[ -f "$notes" ]] || { echo "notes file not found: $notes" >&2; exit 2; }
prerelease="${PRERELEASE:-false}"
[[ "$prerelease" == true || "$prerelease" == false ]] || { echo "PRERELEASE must be true or false" >&2; exit 2; }
(( $# > 0 )) || { echo "no files to upload" >&2; exit 2; }
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY}"
: "${GH_TOKEN:?GH_TOKEN}"

"$(dirname "$0")/release-guard.sh" "$tag" "$repo"

release_json="$(gh api -X POST "repos/$repo/releases" \
  -f tag_name="$tag" -f target_commitish="$sha" -f name="Markise $tag" \
  -F body=@"$notes" -F draft=true -F prerelease="$prerelease" -F generate_release_notes=true)"
id="$(jq -r '.id' <<< "$release_json")"
draft="$(jq -r '.draft' <<< "$release_json")"
[[ "$id" =~ ^[0-9]+$ && "$draft" == true ]] || { echo "unexpected create response: $release_json" >&2; exit 1; }
echo "created draft release id=$id for $tag ($sha)"

for f in "$@"; do
  name="$(basename "$f")"
  echo "uploading $name"
  # uploads.github.com rejects an existing asset name with 422 instead of replacing it
  curl -sS --fail-with-body --retry 3 -X POST \
    -H "Authorization: Bearer $GH_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    -H "Content-Type: application/octet-stream" \
    --data-binary "@$f" \
    "https://uploads.github.com/repos/$repo/releases/$id/assets?name=$name" > /dev/null
done
echo "draft release $id ready: $(jq -r '.html_url' <<< "$release_json")"
