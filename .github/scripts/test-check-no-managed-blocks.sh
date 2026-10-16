#!/bin/sh
# Regression cases for check-no-managed-blocks.sh. Each case builds a throwaway
# directory and asserts the checker's exit code: 0 clean, 1 marker, 2 error.
# Usage: test-check-no-managed-blocks.sh   (run from the repository root)
set -eu
checker="$(pwd)/.github/scripts/check-no-managed-blocks.sh"
[ -x "$checker" ] || { echo "error: run from the repository root; $checker not found" >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# The marker is assembled at run time so this file never contains it.
marker_line() { printf '<!-- BEGIN %s %s: %s -->\n' "OpenRig MANAGED" "BLOCK" "$1"; }
new_repo() {
    git init -q "$1"
    printf '# guide\n' > "$1/AGENTS.md"
    git -C "$1" add AGENTS.md
    git -C "$1" -c user.name=ci -c user.email=ci@localhost commit -qm base
}
failures=0
expect() { # expect <code> <name> <command...>
    want=$1; name=$2; shift 2
    got=0
    "$@" > "$work/out" 2>&1 || got=$?
    if [ "$got" -eq "$want" ]; then
        echo "pass: $name (exit $got)"
    else
        echo "FAIL: $name: expected exit $want, got $got"; sed 's/^/    /' "$work/out"
        failures=$((failures + 1))
    fi
}

new_repo "$work/clean"
expect 0 "clean repository" "$checker" "$work/clean"

new_repo "$work/committed"
marker_line committed >> "$work/committed/AGENTS.md"
git -C "$work/committed" -c user.name=ci -c user.email=ci@localhost commit -qam marker
expect 1 "committed marker" "$checker" "$work/committed"

new_repo "$work/staged"
marker_line staged > "$work/staged/new.md"
git -C "$work/staged" add new.md
expect 1 "staged marker in a new file" "$checker" "$work/staged"

new_repo "$work/local"
marker_line untracked > "$work/local/CLAUDE.md"
git -C "$work/local" update-index --skip-worktree AGENTS.md
marker_line skip-worktree >> "$work/local/AGENTS.md"
expect 0 "markers only in untracked and skip-worktree local files" "$checker" "$work/local"

mkdir "$work/not-a-repo"
expect 2 "directory that is not a repository" "$checker" "$work/not-a-repo"

expect 2 "git failure (GIT_DIR points nowhere)" env GIT_DIR="$work/missing" "$checker" "$work/clean"

expect 2 "missing directory" "$checker" "$work/does-not-exist"

if [ "$failures" -ne 0 ]; then
    echo "error: $failures regression case(s) failed" >&2
    exit 1
fi
echo "ok: all regression cases passed"
