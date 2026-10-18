#!/bin/sh
# Regression cases for check-migrations-only-add.sh. Each case builds a throwaway repository and
# asserts the checker's exit code: 0 only additions, 1 a violation, 2 could not check.
# Usage: test-check-migrations-only-add.sh   (run from the repository root)
set -eu
checker="$(pwd)/.github/scripts/check-migrations-only-add.sh"
[ -x "$checker" ] || { echo "error: run from the repository root; $checker not found" >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dir=crates/voidforge-store/migrations
commit() { git -C "$1" add -A && git -C "$1" -c user.name=ci -c user.email=ci@localhost commit -qm "$2"; }
# A repository whose base commit (tag "base") holds migrations 0001 and 0002.
new_repo() {
    git init -q "$1"
    mkdir -p "$1/$dir"
    printf 'CREATE TABLE a (x int);\n' > "$1/$dir/0001_a.sql"
    printf 'CREATE TABLE b (x int);\n' > "$1/$dir/0002_b.sql"
    printf '# guide\n' > "$1/README.md"
    commit "$1" base
    git -C "$1" -c tag.gpgSign=false tag base
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

new_repo "$work/same"
expect 0 "no change" "$checker" base "$work/same"

new_repo "$work/added"
printf 'CREATE TABLE c (x int);\n' > "$work/added/$dir/0003_c.sql"
commit "$work/added" add
expect 0 "a committed migration after the last" "$checker" base "$work/added"

new_repo "$work/other"
printf 'changed\n' > "$work/other/README.md"
commit "$work/other" other
expect 0 "changes outside the migrations" "$checker" base "$work/other"

new_repo "$work/modified"
printf 'CREATE TABLE a (x bigint);\n' > "$work/modified/$dir/0001_a.sql"
commit "$work/modified" modify
expect 1 "a committed edit to an existing migration" "$checker" base "$work/modified"

new_repo "$work/dirty"
printf -- '-- comment\n' >> "$work/dirty/$dir/0002_b.sql"
expect 1 "an uncommitted edit to an existing migration" "$checker" base "$work/dirty"

new_repo "$work/deleted"
git -C "$work/deleted" rm -q "$dir/0002_b.sql"
commit "$work/deleted" delete
expect 1 "a deleted migration" "$checker" base "$work/deleted"

new_repo "$work/renamed"
git -C "$work/renamed" mv "$dir/0002_b.sql" "$dir/0003_b.sql"
commit "$work/renamed" rename
expect 1 "a renamed migration" "$checker" base "$work/renamed"

new_repo "$work/early"
printf 'CREATE TABLE c (x int);\n' > "$work/early/$dir/0002_c.sql"
commit "$work/early" early
expect 1 "a new migration that does not sort last" "$checker" base "$work/early"

new_repo "$work/unnumbered"
printf 'CREATE TABLE c (x int);\n' > "$work/unnumbered/$dir/c.sql"
commit "$work/unnumbered" unnumbered
expect 1 "a new migration without a version" "$checker" base "$work/unnumbered"

new_repo "$work/untracked-early"
printf 'CREATE TABLE c (x int);\n' > "$work/untracked-early/$dir/0001_c.sql"
expect 1 "an untracked migration that does not sort last" "$checker" base "$work/untracked-early"

new_repo "$work/untracked-ok"
printf 'CREATE TABLE c (x int);\n' > "$work/untracked-ok/$dir/0003_c.sql"
expect 0 "an untracked migration after the last" "$checker" base "$work/untracked-ok"

new_repo "$work/nobase"
expect 2 "no base revision" "$checker" "" "$work/nobase"
expect 2 "an unknown base revision" "$checker" no-such-revision "$work/nobase"

mkdir "$work/not-a-repo"
expect 2 "directory that is not a repository" "$checker" base "$work/not-a-repo"

expect 2 "missing directory" "$checker" base "$work/does-not-exist"

if [ "$failures" -ne 0 ]; then
    echo "error: $failures regression case(s) failed" >&2
    exit 1
fi
