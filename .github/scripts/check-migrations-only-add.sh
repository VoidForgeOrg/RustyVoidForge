#!/bin/sh
# Fail unless the store's migrations only grow: every migration file on the base revision is
# unchanged and still present, and every new file's version sorts after all of the base's
# (Technical Plan, CI/CD: "migrations only ever add"; decision log D-034). The comparison is from
# the base revision to the working tree, so committed and uncommitted edits both count.
# Usage: check-migrations-only-add.sh <base-revision> [repo-dir]   (default repo-dir: .)
# Exit codes: 0 only additions, 1 a migration was changed, removed or added out of order,
# 2 could not check (no base given, not a repository, unknown revision, Git error).
set -eu
dir=crates/voidforge-store/migrations
base=${1:-}
[ -n "$base" ] || { echo "error: no base revision given; could not check migrations" >&2; exit 2; }
cd "${2:-.}" || { echo "error: cannot enter ${2:-.}; could not check migrations" >&2; exit 2; }
git rev-parse --verify --quiet "$base^{commit}" > /dev/null \
    || { echo "error: unknown base revision $base; could not check migrations" >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# --no-renames reports a rename as a deletion plus an addition, so it fails as a deletion.
git diff --name-status --no-renames "$base" -- "$dir" > "$work/diff" \
    || { echo "error: git diff failed; could not check migrations" >&2; exit 2; }
git ls-tree --name-only "$base" -- "$dir/" > "$work/base" \
    || { echo "error: git ls-tree failed; could not check migrations" >&2; exit 2; }
# The highest version on the base: the digits before the first underscore of each file name.
base_max=-1
while IFS= read -r path; do
    v=$(basename "$path" | sed -n 's/^\([0-9][0-9]*\)_.*/\1/p')
    [ -n "$v" ] && [ "$v" -gt "$base_max" ] && base_max=$v
done < "$work/base"
bad=0
while IFS="$(printf '\t')" read -r status path; do
    case $status in
        A)
            v=$(basename "$path" | sed -n 's/^\([0-9][0-9]*\)_.*/\1/p')
            if [ -z "$v" ]; then
                echo "error: $path: a migration name starts with its version, as in 0004_name.sql" >&2; bad=1
            elif [ "$v" -le "$base_max" ]; then
                echo "error: $path: version $v does not sort after the base's highest, $base_max" >&2; bad=1
            else
                echo "added: $path"
            fi ;;
        *)
            echo "error: $path: status $status; an existing migration must never change or disappear" >&2; bad=1 ;;
    esac
done < "$work/diff"
# git diff does not list untracked files; they are additions, and their versions must sort last too.
git ls-files --others --exclude-standard -- "$dir" > "$work/untracked" \
    || { echo "error: git ls-files failed; could not check migrations" >&2; exit 2; }
while IFS= read -r path; do
    v=$(basename "$path" | sed -n 's/^\([0-9][0-9]*\)_.*/\1/p')
    if [ -z "$v" ] || [ "$v" -le "$base_max" ]; then
        echo "error: $path (untracked): its version must sort after the base's highest, $base_max" >&2; bad=1
    fi
done < "$work/untracked"
if [ "$bad" -ne 0 ]; then
    echo "error: migrations only ever add (Technical Plan, CI/CD); write a new migration instead" >&2
    exit 1
fi
echo "ok: migrations under $dir only add, relative to $base"
