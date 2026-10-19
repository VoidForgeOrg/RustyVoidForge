#!/usr/bin/env bash
# The local server (Technical Plan P3.4s; decision log D-050), run by `just serve` against the
# local stack's dev universe.
#
# Owner safety: the only database this script names is voidforge_dev. It never touches the owner's
# voidforge database. serve itself refuses a database without a universe (exit 3) before migrating
# anything, so run `just seed` first.
#
# RULES=fast uses rules/v0-fast.toml instead of rules/v0.toml; the universe must have been seeded
# with the same rules. The token checks use the local dex (D-046's defaults).
set -euo pipefail

db=voidforge_dev
case "${RULES:-v0}" in
    v0) rules=rules/v0.toml ;;
    fast) rules=rules/v0-fast.toml ;;
    *) echo "error: RULES is v0 or fast, not ${RULES}" >&2; exit 2 ;;
esac
[ "$db" = voidforge_dev ] || { echo "error: refusing database $db" >&2; exit 2; }
# The queries are checked against .sqlx/, never against this database (D-034). exec, so Ctrl-C and
# SIGTERM reach serve itself and it stops with its final snapshot.
SQLX_OFFLINE=true cargo build -q -p voidforge-server --locked
VOIDFORGE_DATABASE_URL="postgres://voidforge:voidforge-local-test-password@localhost:5432/$db" \
    exec target/debug/voidforge-server serve --rules "$rules"
