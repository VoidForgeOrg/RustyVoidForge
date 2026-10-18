#!/usr/bin/env bash
# Regression cases for database-lock.sh (decision log D-044), against a real Postgres server:
# the same functions database.sh uses, on a scratch lock named voidforge_test_lock_selftest.
#   1. a free lock is taken and released;
#   2. a live holder makes a run wait, then fail with the clearing command; the lock is kept;
#   3. review-r2's M1, made deterministic: two runs both see a holder that is no longer running.
#      Both must refuse (none enters), and the stale lock must still be there, untouched;
#   4. a release leaves alone a lock whose comment names another holder.
# Environment: PSQL, a psql command for the server (as for database.sh). The CI database job and
# just db-test run this before database.sh. It touches only voidforge_test_lock_selftest.
set -euo pipefail

: "${PSQL:?set PSQL to a psql command for the server}"
read -r -a psql <<< "$PSQL"
lock_db=voidforge_test_lock_selftest
fail() { echo "error: $*" >&2; exit 2; }
real_sql() { "${psql[@]}" -X -q -v ON_ERROR_STOP=1 -At -d postgres -c "$1" < /dev/null; }

# A worker: one contender, as its own process. With PAUSE set, it stops after its first read of
# the lock's owner until the file "$PAUSE/go" exists, having marked "$PAUSE/seen".
if [ "${1:-}" = --worker ]; then
    sql() {
        local result
        result=$(real_sql "$1") || return $?
        if [ -n "${PAUSE:-}" ] && [ ! -e "$PAUSE/seen" ] && [[ $1 == *shobj_description* ]]; then
            touch "$PAUSE/seen"
            until [ -e "$PAUSE/go" ]; do sleep 0.05; done
        fi
        printf '%s\n' "$result"
    }
    # shellcheck source=database-lock.sh
    . "$(dirname "$0")/database-lock.sh"
    lock_acquire
    echo "entered $$"
    [ -z "${HOLD:-}" ] || until [ -e "$HOLD" ]; do sleep 0.05; done
    lock_release
    exit 0
fi

sql() { real_sql "$1"; }
# shellcheck source=database-lock.sh
. "$(dirname "$0")/database-lock.sh"
scratch=$(mktemp -d)
trap 'real_sql "DROP DATABASE IF EXISTS $lock_db" > /dev/null; rm -rf "$scratch"' EXIT
real_sql "DROP DATABASE IF EXISTS $lock_db"
bad=0
check() {
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "FAIL: $1: expected '$3', found '$2'"; bad=1; fi
}
exists() { real_sql "SELECT count(*) FROM pg_database WHERE datname = '$lock_db'"; }
worker() { "$0" --worker "$@"; }

# A pid that no longer runs: a finished child's.
sleep 0 & dead=$!
wait "$dead"

echo "--- 1. a free lock is taken and released"
out=$(worker 2>&1) && code=0 || code=$?
check "the run enters and exits 0" "$code $(grep -c '^entered' <<< "$out")" "0 1"
check "the lock is gone after release" "$(exists)" 0

echo "--- 2. a live holder: wait, then fail with the clearing command"
sleep 60 & live=$!
real_sql "CREATE DATABASE $lock_db"
real_sql "COMMENT ON DATABASE $lock_db IS '$(hostname) $live'"
out=$(VOIDFORGE_DB_LOCK_WAIT=0 worker 2>&1) && code=0 || code=$?
check "exit 2, not entered" "$code $(grep -c '^entered' <<< "$out")" "2 0"
check "names the holder and the clearing command" \
    "$(grep -c "still holds $lock_db.*$(lock_clear_command)" <<< "$out")" 1
check "the holder's lock is kept" "$(lock_owner)" "$(hostname) $live"
kill "$live"; wait "$live" 2> /dev/null || true
real_sql "DROP DATABASE $lock_db"

echo "--- 3. two runs both see a holder that is no longer running (review-r2 M1)"
real_sql "CREATE DATABASE $lock_db"
real_sql "COMMENT ON DATABASE $lock_db IS '$(hostname) $dead'"
mkdir "$scratch/a" "$scratch/b"
PAUSE=$scratch/a HOLD=$scratch/a/end worker > "$scratch/a.log" 2>&1 & a=$!
PAUSE=$scratch/b HOLD=$scratch/b/end worker > "$scratch/b.log" 2>&1 & b=$!
for w in a b; do
    for _ in $(seq 600); do [ -e "$scratch/$w/seen" ] && break; sleep 0.05; done
    check "$w read the stale owner" "$([ -e "$scratch/$w/seen" ] && echo yes)" yes
done
# A goes first, then B, as in review-r2's interleaving; neither may take the lock over.
touch "$scratch/a/go"; wait "$a" && ca=0 || ca=$?
touch "$scratch/b/go"; wait "$b" && cb=0 || cb=$?
touch "$scratch/a/end" "$scratch/b/end"
check "both refuse with exit 2" "$ca $cb" "2 2"
check "neither entered" "$(cat "$scratch/a.log" "$scratch/b.log" | grep -c '^entered')" 0
check "both name the clearing command" \
    "$(cat "$scratch/a.log" "$scratch/b.log" | grep -c "no longer running.*$(lock_clear_command)")" 2
check "the stale lock is untouched" "$(lock_owner)" "$(hostname) $dead"
real_sql "DROP DATABASE $lock_db"

echo "--- 4. a release leaves another holder's lock alone"
lock_acquire
real_sql "COMMENT ON DATABASE $lock_db IS 'elsewhere 1'"
lock_release 2> "$scratch/release.log"
check "the lock is kept, with a warning" "$(exists) $(grep -c 'left alone' "$scratch/release.log")" "1 1"

[ "$bad" -eq 0 ] && echo "ok: every database-lock case passed"
exit "$bad"
