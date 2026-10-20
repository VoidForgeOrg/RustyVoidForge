#!/bin/sh
# Fail unless deploy/dex/config.ci.yaml is deploy/dex/config.yaml plus exactly the password grant
# for the static test users (decision log D-033's carry, D-052): the lines `    - password` and
# `  passwordConnector: local`, once each. Comments and blank lines are ignored, so each file
# keeps its own header and notes. Anything else that differs is drift between local and CI.
# Usage: check-dex-parity.sh [dex-dir]   (default: deploy/dex)
# Exit codes: 0 in parity, 1 drift, 2 could not check (a file is missing or unreadable).
set -eu
dir="${1:-deploy/dex}"
local_config="$dir/config.yaml"
ci_config="$dir/config.ci.yaml"
for f in "$local_config" "$ci_config"; do
    [ -r "$f" ] || { echo "error: cannot read $f; could not check dex parity" >&2; exit 2; }
done
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Settings only: no comment lines, no blank lines.
settings() { grep -v -E '^[[:space:]]*(#|$)' "$1" || true; }
settings "$local_config" > "$work/local"
settings "$ci_config" > "$work/ci"
failed=0
for line in '    - password' '  passwordConnector: local'; do
    n=$(grep -c -x -F -e "$line" "$work/ci" || true)
    if [ "$n" -ne 1 ]; then
        echo "error: $ci_config has \"$line\" $n times; the CI config adds it exactly once" >&2
        failed=1
    fi
    if grep -q -x -F -e "$line" "$work/local"; then
        echo "error: $local_config has \"$line\"; the password grant is CI only" >&2
        failed=1
    fi
done
grep -v -x -F -e '    - password' -e '  passwordConnector: local' "$work/ci" > "$work/ci-less" || true
if ! diff -u "$work/local" "$work/ci-less" > "$work/diff"; then
    echo "error: $ci_config differs from $local_config beyond the password grant:" >&2
    sed 's/^/    /' "$work/diff" >&2
    failed=1
fi
if [ "$failed" -ne 0 ]; then
    exit 1
fi
echo "ok: $ci_config is $local_config plus exactly the password grant"
