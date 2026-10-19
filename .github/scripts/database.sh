#!/usr/bin/env bash
# The Database tests stage (Technical Plan, CI/CD; decision log D-034): the migrations applied to an
# empty database and re-run as a no-op, the sqlx offline metadata check, and the Postgres tests of
# store and host. The CI `database` job and `just db-test` both run this script.
#
# Environment:
#   DATABASE_URL  the dedicated test database; its name must be voidforge_test. #[sqlx::test] keeps
#                 its bookkeeping there and creates one _sqlx_test_* database per test beside it.
#   PSQL          a psql command for the same server, run against its maintenance database
#                 (CI: docker exec into the service container; just db-test: docker compose exec).
# Tools: sqlx-cli and cargo-nextest at the versions in docs/versions.md.
#
# The script only ever creates voidforge_test and its lock database voidforge_test_lock, and only
# ever drops that lock and databases whose names start with _sqlx_test_. It never connects to any
# other database.
#
# A shared local server (phase 2 gate, QA O1): several agents may run this at once on one `just up`
# stack, where force-dropping every _sqlx_test_* database killed another seat's live run. So:
# - one run at a time per server: a run holds voidforge_test_lock (database-lock.sh) and waits up
#   to VOIDFORGE_DB_LOCK_WAIT seconds (default 1800) for another run's. A lock whose holder (host
#   and pid in its comment) is no longer running is refused with the command that clears it,
#   never taken over (test-database-lock.sh is the regression);
# - cleanup never terminates a session: it drops idle _sqlx_test_* databases before and after the
#   run, refuses to start while one is in use (a test run outside this script, such as a direct
#   cargo nextest against voidforge_test), and after the run leaves one in use with a warning.
# Test database names are fixed per test path, so concurrent runs of the same test collide even
# without this script: the later one fails. Run Postgres tests through `just db-test`.
set -euo pipefail

fail() { echo "error: $*" >&2; exit 2; }

: "${DATABASE_URL:?set DATABASE_URL to the voidforge_test database}"
: "${PSQL:?set PSQL to a psql command for the same server}"
read -r -a psql <<< "$PSQL"
command -v sqlx > /dev/null || fail "sqlx-cli not found (docs/versions.md)"
command -v cargo-nextest > /dev/null || fail "cargo-nextest not found (docs/versions.md)"

# Everything up to the last slash is the server; the rest is the database name, without a query.
server=${DATABASE_URL%/*}
name=${DATABASE_URL##*/}
name=${name%%\?*}
[ "$name" = voidforge_test ] \
    || fail "DATABASE_URL names database '$name'; this script only runs against voidforge_test"

migrations=crates/voidforge-store/migrations
cli_db=_sqlx_test_cli_migrate

sql() { "${psql[@]}" -X -q -v ON_ERROR_STOP=1 -At -d postgres -c "$1" < /dev/null; }

# --- One run at a time per server
lock_db=voidforge_test_lock
# shellcheck source=database-lock.sh
. "$(dirname "$0")/database-lock.sh"

# Drops idle _sqlx_test_* databases (and only those), never terminating a session, then clears
# sqlx's bookkeeping rows for databases that are gone. With `before`, a database in use is an
# error: another test run is using this server. After the run, it is left with a warning.
cleanup() {
    local when=$1 db busy
    busy=$(sql "SELECT DISTINCT datname FROM pg_stat_activity WHERE left(datname, 11) = '_sqlx_test_'")
    if [ -n "$busy" ] && [ "$when" = before ]; then
        fail "test databases in use by another session (a test run outside this script?): $busy"
    fi
    for db in $(sql "SELECT datname FROM pg_database WHERE left(datname, 11) = '_sqlx_test_'"); do
        if grep -qx "$db" <<< "$busy"; then
            echo "warning: left test database $db, in use by another session" >&2
            continue
        fi
        sql "DROP DATABASE IF EXISTS \"$db\""
        echo "dropped test database $db"
    done
    "${psql[@]}" -X -q -v ON_ERROR_STOP=1 -At -d voidforge_test < /dev/null -c \
        "DO \$\$ BEGIN IF to_regclass('_sqlx_test.databases') IS NOT NULL THEN
             DELETE FROM _sqlx_test.databases
             WHERE db_name NOT IN (SELECT datname FROM pg_database); END IF; END \$\$"
}

if [ -z "$(sql "SELECT 1 FROM pg_database WHERE datname = 'voidforge_test'")" ]; then
    sql "CREATE DATABASE voidforge_test" 2> /dev/null \
        || [ -n "$(sql "SELECT 1 FROM pg_database WHERE datname = 'voidforge_test'")" ] \
        || fail "cannot create voidforge_test"
    echo "created voidforge_test"
fi
trap 'status=$?; lock_release || { echo "error: releasing $lock_db failed" >&2; status=2; }; exit $status' EXIT
lock_acquire
cleanup before
trap 'status=$?; cleanup after || { echo "error: cleanup failed" >&2; status=2; }
      lock_release || { echo "error: releasing $lock_db failed" >&2; status=2; }; exit $status' EXIT

echo "--- migrations on an empty database, then a second run"
sql "CREATE DATABASE \"$cli_db\""
cli_url="$server/$cli_db"
applied() {
    "${psql[@]}" -X -q -v ON_ERROR_STOP=1 -At -d "$cli_db" < /dev/null -c \
        "SELECT version || ' ' || success || ' ' || installed_on FROM _sqlx_migrations ORDER BY version"
}
sqlx migrate run --source "$migrations" --database-url "$cli_url"
first=$(applied)
files=$(find "$migrations" -name '*.sql' | wc -l | tr -d ' ')
[ "$(grep -c ' true ' <<< "$first")" -eq "$files" ] \
    || fail "expected $files successful migrations, found: $first"
second_output=$(sqlx migrate run --source "$migrations" --database-url "$cli_url")
[ -z "$second_output" ] || fail "the second run applied something: $second_output"
[ "$(applied)" = "$first" ] || fail "the second run changed _sqlx_migrations"
echo "ok: $files migrations applied to an empty database; the second run applied nothing"

echo "--- sqlx offline metadata check"
DATABASE_URL="$cli_url" cargo sqlx prepare --workspace --check -- --all-targets

echo "--- store, host and server tests against Postgres"
# Only the Postgres binaries (store's database, host's, replay's and the server's postgres, host's
# crash, the server's accounts and routes): the other ignored tests are the determinism suites' two-process
# runs, the crash harnesses' children and the nightly run.
SQLX_OFFLINE=true cargo nextest run --workspace --run-ignored only --no-fail-fast --locked \
    -E 'binary(=database) | binary(=postgres) | (binary(=crash) & test(=every_retried_command_ends_with_exactly_one_outcome_on_postgres)) | (binary(=accounts) & !test(=accounts_child)) | binary(=routes)'

