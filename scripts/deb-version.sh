#!/usr/bin/env bash
# Debian package version for a release label (SemVer 2.0.0, optional "v").
#
# usage: scripts/deb-version.sh <label>      e.g. 0.1.0-beta.1 -> 0.1.0~beta.1-1
#
# Mapping: the FIRST "-" (SemVer's prerelease separator) becomes "~", which dpkg sorts before
# the end of the string, so 0.1.0~beta.1 < 0.1.0. Later hyphens inside the prerelease stay
# (they are allowed in a Debian upstream version once a revision follows), "+build" stays,
# and the Debian revision "-1" is appended. Precedence then matches SemVer for numeric and
# alphanumeric identifiers, dotted prefixes (alpha < alpha.1 < alpha.beta < beta.2 < beta.11 <
# rc.1) and hyphenated extensions (alpha < alpha-1; 0.1.0-beta.1 < 0.1.0-beta.1-dryrun.x, as in
# SemVer). Known gap: dpkg orders letters before "-" and "-" before ".", SemVer compares such
# identifiers as ASCII strings, so alpha-1 vs alphaz or alpha-1 vs alpha.beta differ; Markise
# tags do not use hyphens inside prerelease identifiers (scripts/test-deb-version.sh checks the
# supported table with dpkg --compare-versions).
set -euo pipefail
label="${1:?label}"
label="${label#v}"
semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
[[ "$label" =~ $semver ]] || { echo "deb-version: '$label' is not a SemVer version" >&2; exit 2; }
base="${label%%+*}"
build=""
[[ "$label" == *+* ]] && build="+${label#*+}"
case "$base" in
  *-*) base="${base%%-*}~${base#*-}" ;;
esac
echo "$base$build-1"
