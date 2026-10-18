#!/usr/bin/env bash
# The Database tests stage (Technical Plan, CI/CD; decision log D-034): the migrations applied to an
# empty database and re-run as a no-op, the sqlx offline metadata check, and the store's Postgres
# tests. The CI `database` job and `just db-test` both run this script.
#
# Environment:
#   DATABASE_URL  the dedicated test database; its name must be voidforge_test. #[sqlx::test] keeps
#                 its bookkeeping there and creates one _sqlx_test_* database per test beside it.
#   PSQL          a psql command for the same server, run against its maintenance database
#                 (CI: docker exec into the service container; just db-test: docker compose exec).
# Tools: sqlx-cli and cargo-nextest at the versions in docs/versions.md.
#
# The script only ever creates voidforge_test, and only ever drops databases whose names start
# with _sqlx_test_. It drops them before and after the run, so nothing accumulates on the server.
# It never connects to any other database.
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

# Drops every database named _sqlx_test_* (and only those), then clears sqlx's bookkeeping rows.
cleanup() {
    local db
    for db in $(sql "SELECT datname FROM pg_database WHERE left(datname, 11) = '_sqlx_test_'"); do
        sql "DROP DATABASE IF EXISTS \"$db\" WITH (FORCE)"
        echo "dropped test database $db"
    done
    "${psql[@]}" -X -q -v ON_ERROR_STOP=1 -At -d voidforge_test < /dev/null -c \
        "DO \$\$ BEGIN IF to_regclass('_sqlx_test.databases') IS NOT NULL THEN
             DELETE FROM _sqlx_test.databases; END IF; END \$\$"
}

if [ -z "$(sql "SELECT 1 FROM pg_database WHERE datname = 'voidforge_test'")" ]; then
    sql "CREATE DATABASE voidforge_test"
    echo "created voidforge_test"
fi
cleanup
trap 'status=$?; cleanup || { echo "error: cleanup failed" >&2; status=2; }; exit $status' EXIT

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

echo "--- store tests against Postgres"
SQLX_OFFLINE=true cargo nextest run -p voidforge-store --run-ignored only --no-fail-fast --locked
