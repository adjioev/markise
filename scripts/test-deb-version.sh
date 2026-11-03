#!/usr/bin/env bash
# Checks scripts/deb-version.sh: known mappings, and that dpkg --compare-versions orders the
# mapped versions of each SemVer pair like SemVer precedence (left < right). Needs dpkg (CI:
# the Linux lint job).
set -euo pipefail
dv="$(dirname "$0")/deb-version.sh"
fail=0

while read -r label want; do
  got="$("$dv" "$label")"
  if [[ "$got" != "$want" ]]; then echo "FAIL map $label: got $got, want $want"; fail=1; fi
done <<'TABLE'
0.1.0 0.1.0-1
v0.1.0 0.1.0-1
0.1.0-beta.1 0.1.0~beta.1-1
0.1.0-beta.1-dryrun.abc1234 0.1.0~beta.1-dryrun.abc1234-1
1.0.0-alpha-1 1.0.0~alpha-1-1
1.0.0+build.5 1.0.0+build.5-1
1.0.0-rc.1+build-7 1.0.0~rc.1+build-7-1
TABLE

# each line: lower higher (SemVer precedence)
while read -r lo hi; do
  a="$("$dv" "$lo")" b="$("$dv" "$hi")"
  if ! dpkg --compare-versions "$a" lt "$b"; then
    echo "FAIL order: $lo < $hi but $a >= $b"; fail=1
  fi
done <<'PAIRS'
1.0.0-alpha 1.0.0-alpha.1
1.0.0-alpha.1 1.0.0-alpha.beta
1.0.0-alpha.beta 1.0.0-beta
1.0.0-beta 1.0.0-beta.2
1.0.0-beta.2 1.0.0-beta.11
1.0.0-beta.11 1.0.0-rc.1
1.0.0-rc.1 1.0.0
1.0.0-alpha 1.0.0-alpha-1
1.0.0-alpha-1 1.0.0-alpha-2
1.0.0-alpha-1 1.0.0-beta
0.1.0-beta.1 0.1.0-beta.1-dryrun.abc1234
0.1.0-beta.1-dryrun.abc1234 0.1.0-beta.2
0.1.0-beta.1-dryrun.abc1234 0.1.0
0.1.0-beta.1 0.1.0-beta.1-abcdef0
0.1.0-beta.9 0.1.0-beta.10
0.1.0 0.1.1-alpha
0.9.0 0.10.0
1.0.0 2.0.0-alpha
PAIRS

for bad in 1.0 01.0.0 1.0.0- 1.0.0-beta_1 "1.0.0 beta"; do
  if "$dv" "$bad" >/dev/null 2>&1; then echo "FAIL: accepted '$bad'"; fail=1; fi
done

(( fail == 0 )) && echo "deb-version: all checks passed"
exit "$fail"
