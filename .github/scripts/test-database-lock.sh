#!/usr/bin/env bash
# Regression cases for database-lock.sh (decision log D-044), against a real Postgres server:
# the same functions database.sh uses, on a scratch lock of this run's own,
# voidforge_test_lock_selftest_<pid>, dropped on exit.
#   1. a free lock is taken and released;
#   2. a live holder makes a run wait, then fail with the clearing command; the lock is kept;
#   3. review-r2's M1, made deterministic: two runs both see a holder that is no longer running.
#      Both must refuse (none enters), and the stale lock must still be there, untouched;
#   4. a release leaves alone a lock whose comment names another holder;
#   5. two runs of this script at once both pass: each has its own scratch lock, so concurrent
#      `just db-test` runs reach the real lock instead of breaking each other's self-test (phase 2
#      gate round 3). A nested run (LOCK_TEST_NESTED set) skips this case.
#   6. the sweep (P5.3, D-066; Phase 3 gate R1-G5): a SIGKILLed run never reaches its EXIT trap and
#      leaves its voidforge_test_lock_selftest_<pid> behind. Every run sweeps first: it drops one
#      whose pid is gone, lists one whose pid is alive with the command that drops it, and never
#      touches a name of any other shape. Planted cases: a dead pid, a live pid, an odd name. A
#      nested run skips this case.
# Every worker runs under a hard timeout (LOCK_TEST_TIMEOUT seconds, default 60), so a lock that
# deadlocks or never returns fails its case by timeout, never hangs the run (a mutant that let a
# second worker in once deadlocked an earlier version of this test).
# Environment: PSQL, a psql command for the server (as for database.sh). The CI database job and
# just db-test run this before database.sh. It touches only voidforge_test_lock_selftest_* databases:
# its own, and the sweep's (pids are this host's: the CI job and the local stack run their checks on
# the server's host).
set -euo pipefail
limit=${LOCK_TEST_TIMEOUT:-60}
# A live holder must never be waited for here: case 2 checks the wait's end with no wait at all.
export VOIDFORGE_DB_LOCK_WAIT=0
# Runs a command under a hard timeout (macOS has no timeout(1)); SIGALRM ends it, exit 142.
with_timeout() { perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$limit" "$@"; }
# Polls a condition for up to the limit; false if it never held.
await() {
    local tries=$((limit * 20))
    until "$@"; do
        tries=$((tries - 1))
        [ "$tries" -gt 0 ] || return 1
        sleep 0.05
    done
}

: "${PSQL:?set PSQL to a psql command for the server}"
read -r -a psql <<< "$PSQL"
# Workers share their parent's scratch lock; every top-level run gets its own.
if [ "${1:-}" = --worker ]; then
    lock_db=$LOCK_TEST_DB
else
    lock_db=voidforge_test_lock_selftest_$$
    export LOCK_TEST_DB=$lock_db
fi
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
            await test -e "$PAUSE/go" || fail "timed out waiting to go"
        fi
        printf '%s\n' "$result"
    }
    # shellcheck source=database-lock.sh
    . "$(dirname "$0")/database-lock.sh"
    lock_acquire
    echo "entered $$"
    [ -z "${HOLD:-}" ] || await test -e "$HOLD" || fail "timed out holding the lock"
    lock_release
    exit 0
fi

sql() { real_sql "$1"; }
# shellcheck source=database-lock.sh
. "$(dirname "$0")/database-lock.sh"

# Leftovers of runs that never reached their EXIT trap: drop those whose pid is gone, list those
# whose pid is alive, and leave any other name alone. Never this run's own.
sweep() {
    local old pid out
    for old in $(real_sql "SELECT datname FROM pg_database WHERE datname LIKE 'voidforge\_test\_lock\_selftest\_%' ORDER BY 1"); do
        [ "$old" = "$lock_db" ] && continue
        if ! [[ $old =~ ^voidforge_test_lock_selftest_[0-9]+$ ]]; then
            echo "sweep: $old has an unexpected name; not touching it"
            continue
        fi
        pid=${old##*_}
        if ps -p "$pid" > /dev/null 2>&1; then
            echo "sweep: $old: pid $pid is alive (another run?); if it isn't, drop it with:"
            echo "  $PSQL -d postgres -c 'DROP DATABASE \"$old\"'"
        elif out=$(real_sql "DROP DATABASE IF EXISTS \"$old\"" 2>&1); then
            echo "sweep: dropped $old (pid $pid is gone)"
        else
            echo "sweep: couldn't drop $old (pid $pid is gone; in use?): $out"
        fi
    done
}
sweep

scratch=$(mktemp -d)
trap 'real_sql "DROP DATABASE IF EXISTS $lock_db" > /dev/null; rm -rf "$scratch"' EXIT
real_sql "DROP DATABASE IF EXISTS $lock_db"
bad=0
check() {
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "FAIL: $1: expected '$3', found '$2'"; bad=1; fi
}
exists() { real_sql "SELECT count(*) FROM pg_database WHERE datname = '$lock_db'"; }
worker() { with_timeout "$0" --worker "$@"; }
# A worker in the background has finished, or has entered the lock.
done_or_entered() { ! kill -0 "$1" 2> /dev/null || grep -q '^entered' "$2"; }

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
out=$(worker 2>&1) && code=0 || code=$?
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
    await test -e "$scratch/$w/seen" && seen=yes || seen=no
    check "$w read the stale owner" "$seen" yes
done
# A goes first, then B, as in review-r2's interleaving; neither may take the lock over. A worker
# that entered holds the lock until its end file, so each is awaited to finish or enter (never a
# bare wait), and only then are both ended.
touch "$scratch/a/go"
await done_or_entered "$a" "$scratch/a.log" && in_time=yes || in_time=no
check "a finished or entered in time" "$in_time" yes
touch "$scratch/b/go"
await done_or_entered "$b" "$scratch/b.log" && in_time=yes || in_time=no
check "b finished or entered in time" "$in_time" yes
touch "$scratch/a/end" "$scratch/b/end"
wait "$a" && ca=0 || ca=$?
wait "$b" && cb=0 || cb=$?
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

if [ -z "${LOCK_TEST_NESTED:-}" ]; then
    echo "--- 5. two runs of this script at once both pass (phase 2 gate round 3)"
    LOCK_TEST_NESTED=1 with_timeout "$0" > "$scratch/n1.log" 2>&1 & n1=$!
    LOCK_TEST_NESTED=1 with_timeout "$0" > "$scratch/n2.log" 2>&1 & n2=$!
    wait "$n1" && c1=0 || c1=$?
    wait "$n2" && c2=0 || c2=$?
    check "both exit 0" "$c1 $c2" "0 0"
    [ "$c1 $c2" = "0 0" ] || grep -h FAIL "$scratch/n1.log" "$scratch/n2.log" || true
fi

if [ -z "${LOCK_TEST_NESTED:-}" ]; then
    echo "--- 6. the sweep drops a dead run's leftover, lists a live one's, leaves odd names alone"
    sh -c 'exit 0' & dead_pid=$!
    wait "$dead_pid"
    sleep 300 & live_pid=$!
    dead_db=voidforge_test_lock_selftest_$dead_pid
    live_db=voidforge_test_lock_selftest_$live_pid
    odd_db=voidforge_test_lock_selftest_${$}odd
    planted=("$dead_db" "$live_db" "$odd_db")
    for db in "${planted[@]}"; do real_sql "CREATE DATABASE \"$db\""; done
    sweep > "$scratch/sweep.log"
    gone() { real_sql "SELECT count(*) FROM pg_database WHERE datname = '$1'"; }
    check "a dead run's leftover is dropped" "$(gone "$dead_db")" 0
    check "and the sweep says so" "$(grep -c "dropped $dead_db" "$scratch/sweep.log")" 1
    check "a live run's is kept" "$(gone "$live_db")" 1
    check "and listed with its drop command" "$(grep -c "DROP DATABASE \\\"$live_db" "$scratch/sweep.log")" 1
    check "an odd name is left alone" "$(gone "$odd_db")" 1
    kill "$live_pid" 2> /dev/null || true
    for db in "${planted[@]}"; do real_sql "DROP DATABASE IF EXISTS \"$db\""; done
fi

[ "$bad" -eq 0 ] && echo "ok: every database-lock case passed"
exit "$bad"
