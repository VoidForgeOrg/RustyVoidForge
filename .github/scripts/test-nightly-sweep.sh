#!/usr/bin/env bash
# Regression cases for nightly-sim.sh's leftover sweep (P5.3, D-066; R2-B1), against a real
# Postgres server: liveness comes from the server, never from ps.
#   - a leftover named after a pid that's dead here, held by a live session but with no connection
#     to it (a live run between CREATE and serve, or while its invariants build), is kept and
#     listed with its drop command;
#   - an unheld leftover is dropped;
#   - a name of another shape is left alone;
#   - the run's own holder session ends when it exits.
# It touches only the voidforge_nightly_* databases it plants, and runs only on a server with no
# others.
#
# Usage: PSQL="<psql command for the server>" test-nightly-sweep.sh
# Exit codes: 0 every case passed; 1 one failed; 2 not run (other nightly databases on the server).
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
: "${PSQL:?set PSQL to a psql command for the server, for example: docker exec -i <container> psql -U voidforge}"
read -r -a psql <<< "$PSQL"
real_sql() { "${psql[@]}" -X -q -At -v ON_ERROR_STOP=1 -d postgres -c "$1" < /dev/null; }
gone() { real_sql "SELECT count(*) FROM pg_database WHERE datname = '$1'"; }
dead_pid() { sh -c 'exit 0' & local p=$!; wait "$p"; echo "$p"; }
bad=0
check() {
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "FAIL: $1: expected '$3', found '$2'"; bad=1; fi
}

# The sweep reaches every voidforge_nightly_* database on the server, so a broken sweep under test
# could drop another run's: run only where there's none but ours (CI's database job; locally, a
# private server).
others=$(real_sql "SELECT count(*) FROM pg_database WHERE datname LIKE 'voidforge\_nightly\_%'")
if [ "$others" != 0 ]; then
    echo "error: $others voidforge_nightly_* databases are on this server (another run's?); run this on a server without them"
    exit 2
fi

held_db=voidforge_nightly_20200101000000_$(dead_pid)
unheld_db=voidforge_nightly_20200101000001_$(dead_pid)
odd_db=voidforge_nightly_odd_${$}x
planted=("$held_db" "$unheld_db" "$odd_db")
# Held before it exists, as a run holds its own (QA's F2 on PR A).
"${psql[@]}" -X -q -d "dbname=postgres application_name=$held_db" -c "SELECT pg_sleep(300)" \
    < /dev/null > /dev/null 2>&1 &
holder=$!
for _ in $(seq 1 60); do
    [ "$(real_sql "SELECT count(*) FROM pg_stat_activity WHERE application_name = '$held_db'")" != 0 ] && break
    sleep 0.5
done
for db in "${planted[@]}"; do real_sql "CREATE DATABASE \"$db\"" > /dev/null; done

log=$(mktemp)
NIGHTLY_SWEEP_ONLY=1 NIGHTLY_PSQL="$PSQL" "$here/nightly-sim.sh" > "$log" 2>&1
code=$?
check "the sweep exits 0" "$code" 0
check "an unheld leftover is dropped" "$(gone "$unheld_db")" 0
check "a held one is kept, though its pid is dead here and nothing is connected to it" "$(gone "$held_db")" 1
check "and listed with its drop command" "$(grep -c -F "DROP DATABASE \"$held_db\"" "$log")" 1
check "an odd name is left alone" "$(gone "$odd_db")" 1
own=$(sed -n 's/^nightly-sim: claimed \([a-z0-9_]*\) on the server.*/\1/p' "$log")
check "the run claimed its own name" "${own:+yes}" yes
check "and its holder is gone after it exits" \
    "$(real_sql "SELECT count(*) FROM pg_stat_activity WHERE application_name = '$own'")" 0
[ "$bad" = 0 ] || cat "$log"

real_sql "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE application_name = '$held_db'" > /dev/null || true
kill "$holder" 2> /dev/null || true
for db in "${planted[@]}"; do real_sql "DROP DATABASE IF EXISTS \"$db\"" > /dev/null; done
rm -f "$log"
[ "$bad" -eq 0 ] && echo "ok: every nightly sweep case passed"
exit "$bad"
