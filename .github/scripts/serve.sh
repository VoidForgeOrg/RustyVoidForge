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
# The queries are checked against .sqlx/, never against this database (D-034). `cargo run` finds the
# binary wherever cargo builds it (CARGO_TARGET_DIR included), and on Unix it execs the binary in
# its own place, so after this exec Ctrl-C and SIGTERM reach serve itself, which stops with its
# final snapshot. (Compose runs the binary directly, never `just`: D-050.)
VOIDFORGE_DATABASE_URL="postgres://voidforge:voidforge-local-test-password@localhost:5432/$db" \
    SQLX_OFFLINE=true exec cargo run -q -p voidforge-server --locked -- serve --rules "$rules" \
        --cors-origin http://localhost:8081
