# Voidforge task runner (Technical Plan, Tech stack and CI/CD). The lint, supply-chain and test
# recipes run the same commands as the CI jobs of the same name, so a red check reproduces here.
# db-test and migrate arrived with P2.1 (D-034), bless with P2.6 (D-040), replay with P2.7 (D-041), seed and seed-reset with
# P2.9 (D-043), openapi with P3.1 (D-045), dex-test with P3.2 (D-046), serve with P3.4s (D-050), e2e with P3.6 (D-052), images, images-up, images-down and smoke-images with P5.2 (D-065), nightly-crash and nightly-golden with P5.3 (D-066). Recipes appear with the packages that implement them (decision log D-014).

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
    .github/scripts/check-dex-parity.sh
    .github/scripts/test-check-dex-parity.sh
    python3 .github/scripts/test-web-login-ready.py
    python3 .github/scripts/check-images.py --static
    python3 .github/scripts/check-nightly.py
    python3 .github/scripts/check-nightly.py --self-test
    .github/scripts/test-nightly-issue.sh
    python3 .github/scripts/test-nightly-latency.py
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

# Eight bots play through the API against serve on the fast rules, in a voidforge_nightly_* database
# of the stack's Postgres only, never voidforge; then the log is replayed from its snapshots, the
# core's invariants are checked after every record, and the step latency is read through Grafana.
# Needs a password-grant dex: E2E_ISSUER=http://localhost:5563/dex (a throwaway CI-config dex).
# SIM_MINUTES (30), SIM_SEED; SIM_STEP_P99_MAX and SIM_APPEND_P99_MAX (seconds) bound the latency.
# The nightly's bot simulation (Technical Plan P5.3, D-066).
nightly-sim:
    .github/scripts/nightly-sim.sh

# The file-store seeds run in their own nightly-file-<seed> directories, then the Postgres seeds
# through the database stage: only voidforge_test and its _sqlx_test_* databases, never voidforge.
# VOIDFORGE_CRASH_SEED_BASE (1000), VOIDFORGE_CRASH_SEEDS (1000) and VOIDFORGE_CRASH_PG_SEEDS (100)
# as in CI; a red seed reruns alone with VOIDFORGE_CRASH_SEED_BASE=<seed> VOIDFORGE_CRASH_SEEDS=1.
# The nightly's crash-and-retry run (Technical Plan P5.3, D-066).
nightly-crash:
    DATABASE_URL={{test_db_url}} PSQL="{{compose}} exec -T postgres psql -U voidforge" \
        .github/scripts/nightly-crash.sh

# The three golden checks every PR runs, in a release build. It never blesses: tests/golden must
# be unchanged afterwards.
# The nightly's golden replays (Technical Plan P5.3, D-066).
nightly-golden:
    .github/scripts/nightly-golden.sh

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

# The CI e2e job's scenario (decision log D-052): bots log in through dex, join, build, race to
# colonize and transport cargo, against a fresh universe and the real serve binary; then the
# terminal's device code login (D-053), then the web walk (D-057 to D-062). Needs the CI
# dex config: best E2E_ISSUER naming a throwaway dex with it, or DEX_CONFIG=config.ci.yaml just up
# when the stack's dex is yours to reconfigure. The web walk refuses a taken :8081. It uses
# only its own voidforge_e2e_<time>_<pid> database, dropped on exit, never voidforge.
e2e:
    .github/scripts/e2e.sh

# The server and web images (Technical Plan P5.2, D-065), voidforge-server:local and
# voidforge-web:local, from dockerfiles/Dockerfile, then their checks (user, size, nothing but the
# binary). Nothing is pushed. `just images 1` builds with one job, for a small Docker VM: lower the
# jobs, never resize the VM.
images jobs="2":
    for target in server web; do \
        docker build -f dockerfiles/Dockerfile --target "$target" --build-arg CARGO_BUILD_JOBS={{jobs}} \
            --build-arg VOIDFORGE_GIT_SHA="$(git rev-parse --short=12 HEAD)" -t "voidforge-$target:local" . ; \
    done
    python3 .github/scripts/check-images.py --built

# Both images under compose, healthy (D-065): their own project, voidforge-images, beside the stack,
# never in it (-p pins it: an inherited COMPOSE_PROJECT_NAME would otherwise win). Needs
# VOIDFORGE_DATABASE_URL naming a seeded voidforge_dev or voidforge_e2e_* database; anything else,
# voidforge above all, is refused.
# Exports to the stack's collector unless OTEL_EXPORTER_OTLP_ENDPOINT says otherwise.
images-up:
    db="${VOIDFORGE_DATABASE_URL:?name a seeded scratch database}"; db="${db##*/}"; db="${db%%\?*}"; \
    case "$db" in voidforge_dev|voidforge_e2e_*) ;; *) echo "error: refusing database $db (voidforge_dev or voidforge_e2e_* only)" >&2; exit 2 ;; esac
    VOIDFORGE_WEB_CLIENT_SECRET=voidforge-web-local-test-secret \
    OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT-http://localhost:4318}" \
        docker compose -f deploy/compose.images.yaml -p voidforge-images up --no-build --detach --wait --wait-timeout 360

# Stop and remove the images' project (never the stack's); serve stops with its final snapshot.
images-down:
    VOIDFORGE_DATABASE_URL=unused VOIDFORGE_WEB_CLIENT_SECRET=unused \
        docker compose -f deploy/compose.images.yaml -p voidforge-images down

# Both images under compose, health-gated, then stopped cleanly and taken down (D-065, the
# advisor's A1), on a seeded scratch database: the CI images job's step. Needs `just images` and a
# dex: E2E_ISSUER=http://localhost:5563/dex for a throwaway one. The web binds :8081.
images-compose:
    .github/scripts/images-compose.sh

# `just smoke` against the images (D-065): seed from the server image, serve under compose, the
# web walk against the web image, then `docker compose stop` must end in exit 0. The same needs
# and owner safety as `smoke`.
smoke-images: images
    .github/scripts/e2e.sh --images --smoke

# The smoke test (Technical Plan P4.5, D-062): the e2e job's web walk alone, on the fast rules
# against the compose stack: alice logs in through dex with a cookie jar and plays the MVP loop
# through voidforge-web's forms (build, queue ships, assemble, colonize, transport), each step
# checked against the API's JSON. About two minutes. The same needs and owner safety as `e2e`,
# whose CI run includes this walk.
smoke:
    .github/scripts/e2e.sh --smoke

# The token checks against the real dex (decision log D-046). Needs the CI dex config, which has the
# password grant: DEX_CONFIG=config.ci.yaml just up.
dex-test:
    cargo nextest run -p voidforge-server --test dex --locked --run-ignored only --no-capture

# Writes crates/voidforge-api-types/openapi.json from the API types (decision log D-045). The
# public contract changes only this way, and a reviewer checks every change: call it out in the
# PR body.
openapi:
    VOIDFORGE_BLESS_OPENAPI=1 cargo nextest run -p voidforge-api-types --test openapi_file --locked --run-ignored only -E 'test(=bless)' --no-capture
