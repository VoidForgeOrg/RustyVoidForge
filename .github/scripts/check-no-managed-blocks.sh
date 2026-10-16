#!/bin/sh
# Fail if any tracked file contains an agent-harness managed-block marker.
# Those blocks live only in local checkouts (decision log D-005, D-009).
# Usage: check-no-managed-blocks.sh [repo-dir]   (default: current directory)
set -eu
cd "${1:-.}"
# Built from two halves so this script never matches itself.
marker="OpenRig MANAGED"
marker="$marker BLOCK"
if git grep -n -I -F -e "$marker" -- .; then
    echo "error: tracked files contain agent-harness managed blocks (lines above); keep them local" >&2
    exit 1
fi
echo "ok: no managed-block markers in tracked files"
