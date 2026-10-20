#!/usr/bin/env bash
# The dev universe (Technical Plan P2.9, Local development; decision log D-043), run by
# `just seed` and `just seed-reset` against the local stack.
#
# Owner safety: the only database this script names is voidforge_dev. It never touches the
# owner's voidforge database, and it never overwrites a universe: `seed` refuses (exit 3) when
# voidforge_dev already holds one; `reset` drops and recreates voidforge_dev, and only it, first.
#
# SEED (default 20261018) and PLAYERS (default 100) pick the universe; RULES=fast uses
# rules/v0-fast.toml instead of rules/v0.toml.
set -euo pipefail

mode="${1:-seed}"
db=voidforge_dev
seed="${SEED:-20261018}"
players="${PLAYERS:-100}"
case "${RULES:-v0}" in
    v0) rules=rules/v0.toml ;;
    fast) rules=rules/v0-fast.toml ;;
    *) echo "error: RULES is v0 or fast, not ${RULES}" >&2; exit 2 ;;
esac
psql=(docker compose -f deploy/compose.yaml exec -T postgres psql -U voidforge -X -q -v ON_ERROR_STOP=1 -At)
sql() { "${psql[@]}" -d postgres -c "$1" < /dev/null; }

[ "$db" = voidforge_dev ] || { echo "error: refusing database $db" >&2; exit 2; }
case "$mode" in
    seed) ;;
    reset)
        sql "DROP DATABASE IF EXISTS voidforge_dev WITH (FORCE)"
        echo "dropped voidforge_dev"
        ;;
    *) echo "error: mode is seed or reset" >&2; exit 2 ;;
esac
if [ -z "$(sql "SELECT 1 FROM pg_database WHERE datname = 'voidforge_dev'")" ]; then
    sql "CREATE DATABASE voidforge_dev"
    echo "created voidforge_dev"
fi
# The queries are checked against .sqlx/, never against this database (D-034).
SQLX_OFFLINE=true VOIDFORGE_DATABASE_URL="postgres://voidforge:voidforge-local-test-password@localhost:5432/voidforge_dev" \
    cargo run -q -p voidforge-server --locked -- \
    seed --seed "$seed" --players "$players" --rules "$rules"
