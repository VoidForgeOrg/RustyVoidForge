#!/bin/sh
# Regression cases for check-dex-parity.sh. Each case copies the real dex configs into a
# throwaway directory, changes one thing, and asserts the checker's exit code: 0 in parity,
# 1 drift, 2 could not check.
# Usage: test-check-dex-parity.sh   (run from the repository root)
set -eu
checker="$(pwd)/.github/scripts/check-dex-parity.sh"
[ -x "$checker" ] || { echo "error: run from the repository root; $checker not found" >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
expect() { # expect <code> <name> <command...>
    want=$1; name=$2; shift 2
    got=0
    "$@" > "$work/out" 2>&1 || got=$?
    if [ "$got" -eq "$want" ]; then
        echo "pass: $name (exit $got)"; [ -n "${VERBOSE:-}" ] && sed "s/^/    /" "$work/out"
    else
        echo "FAIL: $name: expected exit $want, got $got"; sed 's/^/    /' "$work/out"
        failures=$((failures + 1))
    fi
}
case_dir() { # case_dir <name>: a copy of the real configs
    mkdir "$work/$1"
    cp deploy/dex/config.yaml deploy/dex/config.ci.yaml "$work/$1/"
    echo "$work/$1"
}
# edit <file> <sed expression>, portable across GNU and BSD sed.
edit() { sed "$2" "$1" > "$1.new" && mv "$1.new" "$1"; }

expect 0 "the repository's own configs" "$checker" deploy/dex

d=$(case_dir comments)
printf '# a new note\n\n' >> "$d/config.ci.yaml"
edit "$d/config.yaml" 's/^# dex for local runs.*/# reworded header/'
expect 0 "comment and blank-line differences only" "$checker" "$d"

d=$(case_dir extra-client)
printf '  - id: extra\n    name: Extra\n    public: true\n' >> "$d/config.ci.yaml"
expect 1 "a client only in the CI config" "$checker" "$d"

d=$(case_dir changed-hash)
edit "$d/config.ci.yaml" 's/hash: "\$2y\$10\$Rg/hash: "$2y$10$XX/'
expect 1 "a password hash changed in the CI config" "$checker" "$d"

d=$(case_dir local-drift)
edit "$d/config.yaml" 's/skipApprovalScreen: true/skipApprovalScreen: false/'
expect 1 "a setting changed in the local config" "$checker" "$d"

d=$(case_dir no-grant)
edit "$d/config.ci.yaml" '/^    - password$/d'
expect 1 "the CI config without the password grant" "$checker" "$d"

d=$(case_dir grant-twice)
edit "$d/config.ci.yaml" 's/^  passwordConnector: local$/  passwordConnector: local\
  passwordConnector: local/'
expect 1 "the password connector named twice" "$checker" "$d"

d=$(case_dir local-grant)
edit "$d/config.yaml" 's/^    - urn:ietf:params:oauth:grant-type:device_code$/    - urn:ietf:params:oauth:grant-type:device_code\
    - password/'
expect 1 "the password grant in the local config" "$checker" "$d"

d=$(case_dir missing)
rm "$d/config.ci.yaml"
expect 2 "a missing config" "$checker" "$d"

if [ "$failures" -ne 0 ]; then
    echo "error: $failures dex parity case(s) failed" >&2
    exit 1
fi
echo "ok: every dex parity case behaved as intended"
