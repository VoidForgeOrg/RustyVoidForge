# Voidforge task runner (Technical Plan, Tech stack and CI/CD). The lint, supply-chain and test
# recipes run the same commands as the CI jobs of the same name, so a red check reproduces here.
# db-test and migrate arrived with P2.1 (D-034), bless with P2.6 (D-040), replay with P2.7 (D-041), seed and seed-reset with
# P2.9 (D-043), openapi with P3.1 (D-045), dex-test with P3.2 (D-046), serve with P3.4s (D-050). Recipes appear with the packages that implement them: e2e (P3.6) does not exist yet (decision log D-014).

set shell := ["bash", "-euo", "pipefail", "-c"]

compose := "docker compose -f deploy/compose.yaml"
test_db_url := "postgres://voidforge:voidforge-local-test-password@localhost:5432/voidforge_test"

# List the recipes.
default:
    @just --list

# DEX_CONFIG=config.ci.yaml selects the CI dex config (password grant on).
# Start postgres, dex and otel-lgtm, wait until healthy, then verify them by effect.
up:
    {{compose}} up --build --detach --wait --wait-timeout 300
    python3 deploy/verify_stack.py

# Verify the running stack without starting anything.
status:
    python3 deploy/verify_stack.py

# Prove telemetry by effect: every binary's stub span is found through Grafana's trace backend.
trace-check: up
    python3 deploy/verify_trace.py

# Stop the stack. Postgres data stays in the voidforge_postgres-data volume.
down:
    {{compose}} down

# Follow service logs, for example `just logs dex`.
logs *services:
    {{compose}} logs --follow {{services}}

# The CI lint job.
lint:
    .github/scripts/check-no-managed-blocks.sh
    .github/scripts/test-check-no-managed-blocks.sh
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    python3 tests/guardrails/check_no_floats.py
    python3 tests/guardrails/check_web_deps.py
    python3 tests/guardrails/run_fixtures.py

# The CI supply-chain job.
supply-chain:
    cargo deny check

# The CI test job.
test:
    cargo nextest run --workspace --locked

# It uses only the dedicated voidforge_test database and the _sqlx_test_* databases it creates
# and drops, never voidforge (decision log D-034).
# The CI database job, against the local stack.
db-test:
    .github/scripts/check-migrations-only-add.sh origin/main
    .github/scripts/test-check-migrations-only-add.sh
    PSQL="{{compose}} exec -T postgres psql -U voidforge" .github/scripts/test-database-lock.sh
    DATABASE_URL={{test_db_url}} PSQL="{{compose}} exec -T postgres psql -U voidforge" \
        .github/scripts/database.sh

# There is no default database: the target is always explicit (decision log D-034).
# Apply the migrations with sqlx-cli to the database DATABASE_URL names.
migrate:
    @test -n "${DATABASE_URL:-}" || { echo "error: set DATABASE_URL to the database to migrate" >&2; exit 2; }
    sqlx migrate run --source crates/voidforge-store/migrations

# The CI determinism job: the suite, then the two-process double run (decision log D-024).
determinism:
    .github/scripts/determinism.sh

# The replay tool (decision log D-041), for example `just replay golden tests/golden` or
# `just replay postgres --rules rules/v0.toml --build <id>`. Exit codes: 0 verified, 1 diverged,
# 2 usage, 3 refused, 4 input error, 5 telemetry.
replay *args:
    cargo run -q -p voidforge-replay --locked -- {{args}}

# Bootstraps the dev universe in the local stack's voidforge_dev database, never voidforge
# (decision log D-043): SEED (20261018) and PLAYERS (100) on Rules v0, or RULES=fast. Refuses
# (exit 3) when voidforge_dev already holds a universe, and prints that universe's hash.
seed:
    .github/scripts/seed.sh seed

# Drops and recreates voidforge_dev (and only it), then seeds. The only reset.
seed-reset:
    .github/scripts/seed.sh reset

# Serves the API over voidforge_dev, never voidforge (decision log D-050), on localhost:8080 with
# the local dex. Ctrl-C stops it with a final snapshot. RULES=fast uses rules/v0-fast.toml.
serve:
    .github/scripts/serve.sh

# Records every golden scenario through the host into tests/golden/ (decision log D-040). Golden
# files change only this way, and a reviewer checks every change: call it out in the PR body.
bless:
    VOIDFORGE_BLESS=1 cargo nextest run -p voidforge-host --test golden --locked --run-ignored only -E 'test(=bless)' --no-capture

# The token checks against the real dex (decision log D-046). Needs the CI dex config, which has the
# password grant: DEX_CONFIG=config.ci.yaml just up.
dex-test:
    cargo nextest run -p voidforge-server --test dex --locked --run-ignored only --no-capture

# Writes crates/voidforge-api-types/openapi.json from the API types (decision log D-045). The
# public contract changes only this way, and a reviewer checks every change: call it out in the
# PR body.
openapi:
    VOIDFORGE_BLESS_OPENAPI=1 cargo nextest run -p voidforge-api-types --test openapi_file --locked --run-ignored only -E 'test(=bless)' --no-capture
