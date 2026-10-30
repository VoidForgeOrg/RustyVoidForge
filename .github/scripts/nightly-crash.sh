#!/usr/bin/env bash
# The nightly's crash-and-retry run (Technical Plan P5.3; decision log D-042, D-066): the file-store
# seeds (crash_nightly, in their own nightly-file-<seed> directories), then the Postgres seeds
# (crash_nightly_postgres) through the database stage's guard and lock (`database.sh
# --nightly-crash`: voidforge_test and its _sqlx_test_* databases only). The CI nightly's `crash`
# job and `just nightly-crash` both run this script.
#
# Environment:
#   VOIDFORGE_CRASH_SEED_BASE   the first seed (default 1000); a failure names its seed, and
#                               VOIDFORGE_CRASH_SEED_BASE=<seed> VOIDFORGE_CRASH_SEEDS=1 reruns it
#   VOIDFORGE_CRASH_SEEDS       file-store seeds (default 1000), VOIDFORGE_CRASH_COMMANDS each (60)
#   VOIDFORGE_CRASH_PG_SEEDS    Postgres seeds (default 100), VOIDFORGE_CRASH_PG_COMMANDS each (30)
#   DATABASE_URL, PSQL          as database.sh: the voidforge_test database, and psql for its server
#   NIGHTLY_LOG_DIR             where the logs go (default target/nightly/crash)
#   GITHUB_STEP_SUMMARY         the job summary to append to (default: stdout)
#   NIGHTLY_READY_WAIT          seconds Postgres may take to answer (default 120; nightly-ready.sh)
# Exit codes: 0 passed; 1 a check failed (a defect, or the build failed); 3 Postgres or the database
# stage wasn't ready (infrastructure: no verdict on the code).
set -uo pipefail

export VOIDFORGE_CRASH_SEED_BASE="${VOIDFORGE_CRASH_SEED_BASE:-1000}"
export VOIDFORGE_CRASH_SEEDS="${VOIDFORGE_CRASH_SEEDS:-1000}"
export VOIDFORGE_CRASH_PG_SEEDS="${VOIDFORGE_CRASH_PG_SEEDS:-100}"
# DATABASE_URL is set for the database stage, so cargo would compile sqlx's macros against
# voidforge_test, which has no tables: the queries build from the committed offline metadata.
export SQLX_OFFLINE=true
: "${DATABASE_URL:?set DATABASE_URL to the voidforge_test database (see database.sh)}"
: "${PSQL:?set PSQL to a psql command for the same server (see database.sh)}"
read -r -a psql <<< "$PSQL"
here="$(cd "$(dirname "$0")" && pwd)"
log_dir="${NIGHTLY_LOG_DIR:-target/nightly/crash}"
mkdir -p "$log_dir"
rm -f "$log_dir/file.log" "$log_dir/postgres.log"
say() { echo "nightly-crash: $*" >&2; }
# shellcheck source=nightly-ready.sh
. "$here/nightly-ready.sh"

finish() {
    local outcome=$1 code=$2
    # Locally the summary goes to stdout itself: reopening /dev/stdout would write at its own offset
    # in a redirected log, under the shell's later lines.
    if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then
        python3 "$here/nightly-summary.py" crash "$outcome" "$log_dir/file.log" "$log_dir/postgres.log" >> "$GITHUB_STEP_SUMMARY"
    else
        python3 "$here/nightly-summary.py" crash "$outcome" "$log_dir/file.log" "$log_dir/postgres.log"
    fi
    say "$outcome"
    exit "$code"
}

say "file store: seeds $VOIDFORGE_CRASH_SEED_BASE..$((VOIDFORGE_CRASH_SEED_BASE + VOIDFORGE_CRASH_SEEDS))"
cargo nextest run -p voidforge-host --test crash --locked --run-ignored only --no-capture \
    -E 'test(=crash_nightly)' 2>&1 | tee "$log_dir/file.log"
file_status=${PIPESTATUS[0]}

# Both stores run even when the first fails: the night reports everything it can. A defect in
# either outranks Postgres not being ready.
# Readiness waits, bounded (nightly-ready.sh): the service is healthy before the job's steps, but a
# night never rests on one probe.
postgres_answers() { timeout 15 "${psql[@]}" -X -q -At -d postgres -c 'SELECT 1' < /dev/null > /dev/null; }
if ! ready_within "$ready_wait" 2 postgres_answers; then
    say "Postgres isn't reachable through PSQL (waited $waited s)"
    pg_status=2
else
    say "Postgres: seeds $VOIDFORGE_CRASH_SEED_BASE..$((VOIDFORGE_CRASH_SEED_BASE + VOIDFORGE_CRASH_PG_SEEDS))"
    "$here/database.sh" --nightly-crash 2>&1 | tee "$log_dir/postgres.log"
    pg_status=${PIPESTATUS[0]}
fi
# database.sh's own refusals (the guard, the lock, a database in use) and psql's connection
# failures exit 2: the stage didn't run. Anything else non-zero is the tests' or the build's.
if [ "$file_status" != 0 ] || { [ "$pg_status" != 0 ] && [ "$pg_status" != 2 ]; }; then
    finish failed 1
elif [ "$pg_status" = 2 ]; then
    finish infra 3
fi
finish passed 0
