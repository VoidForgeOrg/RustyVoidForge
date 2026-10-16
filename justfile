# Voidforge task runner (Technical Plan, Tech stack and CI/CD). The lint, supply-chain and test
# recipes run the same commands as the CI jobs of the same name, so a red check reproduces here.
# Recipes appear with the packages that implement them: e2e (P3.6), replay (P2.7), seed (P2.9)
# and bless (P2.6) do not exist yet (decision log D-014).

set shell := ["bash", "-euo", "pipefail", "-c"]

compose := "docker compose -f deploy/compose.yaml"

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
    # TODO(P1.1): drop --no-tests=warn with the first real test (decision log D-004).
    cargo nextest run --workspace --locked --no-tests=warn
