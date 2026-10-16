#!/bin/sh
# Fail if any tracked file contains an agent-harness managed-block marker.
# Those blocks live only in local checkouts (decision log D-005, D-009).
# Usage: check-no-managed-blocks.sh [repo-dir]   (default: current directory)
# Exit codes: 0 no markers, 1 markers found, 2 could not check (not a repository, Git error).
set -eu
cd "${1:-.}" || { echo "error: cannot enter ${1:-.}; could not check tracked files" >&2; exit 2; }
# Built from two halves so this script never matches itself.
marker="OpenRig MANAGED"
marker="$marker BLOCK"
# git grep exits 0 on a match, 1 on no match, and anything else on an error.
# An error must fail the check, never pass it.
set +e
git grep -n -I -F -e "$marker" -- .
rc=$?
set -e
case $rc in
    0) echo "error: tracked files contain agent-harness managed blocks (lines above); keep them local" >&2
       exit 1 ;;
    1) echo "ok: no managed-block markers in tracked files" ;;
    *) echo "error: git grep failed (exit $rc); could not check tracked files" >&2
       exit 2 ;;
esac
