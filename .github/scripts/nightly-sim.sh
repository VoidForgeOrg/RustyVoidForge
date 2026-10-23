#!/usr/bin/env bash
# The nightly bot simulation (Technical Plan P5.3; decision log D-066). Against the stack's
# Postgres, dex (with the password grant) and collector:
#   1. checks dex grants bot1 a token, Postgres answers, and the collector's Prometheus answers;
#   2. builds serve, the bots and the replay tool (release, against .sqlx/, never a database);
#   3. creates this run's scratch database, voidforge_nightly_<time>_<pid> (only that prefix; a
#      leftover from a killed run is dropped only when the pid in its name is dead), and seeds a
#      universe for eight players on the fast rules;
#   4. starts serve on it (exporting to the collector as service.instance.id=<run>);
#   5. runs `voidforge-bots sim` for SIM_MINUTES (default 30: about 10.7 v0 weeks);
#   6. stops serve with one SIGTERM, which must exit 0;
#   7. verifies the log with `voidforge-replay postgres` from the earliest snapshot (every later
#      snapshot must equal the replay up to it) and from the latest;
#   8. checks the core's invariants after every record (nightly_invariants);
#   9. reads the step and append latency through Grafana's Prometheus (nightly-latency.py), bounded
#      by SIM_STEP_P99_MAX and SIM_APPEND_P99_MAX in seconds when set;
#  10. drops the database and writes the summary.
# It never touches voidforge or any database but its own.
#
# Environment: SIM_MINUTES (30), SIM_SEED (default: the time), E2E_ISSUER (default
# http://localhost:5556/dex; it needs the password grant, so locally a throwaway CI-config dex),
# OTEL_EXPORTER_OTLP_ENDPOINT (default http://localhost:4318), NIGHTLY_PROMETHEUS (see
# nightly-latency.py), NIGHTLY_LOG_DIR (default target/nightly/sim), GITHUB_STEP_SUMMARY.
# Exit codes: 0 passed; 1 a check failed (a defect); 3 not ready (dex, Postgres, the collector or
# serve): infrastructure, no verdict on the code.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
minutes="${SIM_MINUTES:-30}"
seed="${SIM_SEED:-$(date +%s)}"
issuer="${E2E_ISSUER:-http://localhost:5556/dex}"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://localhost:4318}"
export SQLX_OFFLINE=true
prometheus="${NIGHTLY_PROMETHEUS:-http://localhost:3000/api/datasources/proxy/uid/prometheus}"
export NIGHTLY_PROMETHEUS="$prometheus"
log_dir="${NIGHTLY_LOG_DIR:-target/nightly/sim}"
prefix=voidforge_nightly_
run="$(date -u +%Y%m%d%H%M%S)_$$"
db="${prefix}${run}"
url_base="postgres://voidforge:voidforge-local-test-password@localhost:5432"
psql=(docker compose -f deploy/compose.yaml exec -T postgres psql -U voidforge -X -q -v ON_ERROR_STOP=1 -At -d postgres)
[ -n "${NIGHTLY_PSQL:-}" ] && read -r -a psql <<< "$NIGHTLY_PSQL -X -q -v ON_ERROR_STOP=1 -At -d postgres"
sql() { "${psql[@]}" -c "$1" < /dev/null; }
say() { echo "nightly-sim: $*" >&2; }
mkdir -p "$log_dir"
rm -f "$log_dir"/*.log "$log_dir"/*.json
outcome=failed
why=""

summary() {
    if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then
        python3 "$here/nightly-summary.py" sim "$outcome" "$log_dir" "$why" >> "$GITHUB_STEP_SUMMARY"
    else
        python3 "$here/nightly-summary.py" sim "$outcome" "$log_dir" "$why"
    fi
}

serve_pid=
created=0
cleanup() {
    local code=$?
    if [ -n "$serve_pid" ] && kill -0 "$serve_pid" 2> /dev/null; then
        kill -KILL "$serve_pid" 2> /dev/null || true
        wait "$serve_pid" 2> /dev/null || true
    fi
    if [ "$created" = 1 ]; then
        if out=$(sql "DROP DATABASE IF EXISTS \"$db\"" 2>&1); then
            say "dropped $db; logs in $log_dir"
        else
            say "FAILED to drop $db: ${out//$'\n'/ }. Drop it by hand when nothing holds it."
            [ "$code" != 0 ] || code=1
        fi
    fi
    summary
    exit "$code"
}
trap cleanup EXIT
finish() { outcome=$1; why=$2; say "$outcome: $why"; case "$outcome" in passed) exit 0 ;; infra) exit 3 ;; *) exit 1 ;; esac; }

case "$db" in "$prefix"[0-9]*_[0-9]*) ;; *) say "refusing database $db"; exit 2 ;; esac
case "$minutes" in ''|*[!0-9]*) say "SIM_MINUTES must be whole minutes"; exit 2 ;; esac

# 1. The stack.
grant=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$issuer/token" \
    -d 'grant_type=password&username=bot1%40voidforge.test&password=bot1-local-test-password&client_id=voidforge-cli&scope=openid' \
    || true)
[ "$grant" = 200 ] || finish infra "dex at $issuer doesn't grant bot1 a token (HTTP $grant): it needs deploy/dex/config.ci.yaml (locally, a throwaway dex on its own port: E2E_ISSUER)"
sql "SELECT 1" > /dev/null 2>&1 || finish infra "can't reach the stack's Postgres"
curl -sf -o /dev/null "$prometheus/api/v1/query?query=up" || finish infra "the collector's Prometheus isn't reachable at $prometheus"
for old in $(sql "SELECT datname FROM pg_database WHERE datname LIKE 'voidforge\_nightly\_%' ORDER BY 1"); do
    case "$old" in "$prefix"[0-9]*_[0-9]*) ;; *) say "leftover $old has an unexpected name; not touching it"; continue ;; esac
    pid="${old##*_}"
    if ps -p "$pid" > /dev/null 2>&1; then
        say "leftover $old: pid $pid is alive (another run?); not touching it"
    elif sql "DROP DATABASE IF EXISTS \"$old\"" > /dev/null 2>&1; then
        say "dropped leftover $old (pid $pid is gone)"
    else
        say "leftover $old: pid $pid is gone, but it couldn't be dropped"
    fi
done

# 2. The binaries.
cargo build -q --release --locked -p voidforge-server -p voidforge-bots -p voidforge-replay \
    || finish failed "the build failed"
target_dir=$(cargo metadata -q --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
bin="$target_dir/release"

# 3. The universe: one homeworld per bot, on the fast rules.
sql "CREATE DATABASE \"$db\"" > /dev/null || finish infra "couldn't create $db"
created=1
say "created $db"
VOIDFORGE_DATABASE_URL="$url_base/$db" "$bin/voidforge-server" seed --seed 20261023 --players 8 \
    --rules rules/v0-fast.toml > "$log_dir/seed.log" 2>&1 || finish failed "seed failed (see $log_dir/seed.log)"

# 4. serve, as a host process (never through just or cargo run: D-050 I5).
OTEL_RESOURCE_ATTRIBUTES="service.instance.id=$run" VOIDFORGE_DATABASE_URL="$url_base/$db" \
    "$bin/voidforge-server" serve --rules rules/v0-fast.toml --bind 127.0.0.1:0 --issuer "$issuer" \
    > "$log_dir/serve.log" 2>&1 &
serve_pid=$!
addr=
for _ in $(seq 1 300); do
    addr=$(sed -n 's/^ready: listening on \([^,]*\),.*/\1/p' "$log_dir/serve.log")
    [ -n "$addr" ] && break
    kill -0 "$serve_pid" 2> /dev/null || break
    sleep 0.2
done
[ -n "$addr" ] && curl -sf -o /dev/null "http://$addr/openapi.json" || finish infra "serve isn't ready (see $log_dir/serve.log)"
say "serve is ready on $addr (pid $serve_pid); the bots play for $minutes min, seed $seed"
started=$(date +%s)

# 5. The bots.
"$bin/voidforge-bots" sim --api "http://$addr" --issuer "$issuer" --run "n$run" --seed "$seed" \
    --minutes "$minutes" > "$log_dir/sim.out" 2> "$log_dir/bots.log"
bots_code=$?

# 6. One SIGTERM: serve drains and stops with its final snapshot, exit 0 (D-050).
serve_code=
if kill -0 "$serve_pid" 2> /dev/null; then
    kill -TERM "$serve_pid"
    for _ in $(seq 1 300); do kill -0 "$serve_pid" 2> /dev/null || break; sleep 0.1; done
    if kill -0 "$serve_pid" 2> /dev/null; then
        kill -KILL "$serve_pid" 2> /dev/null || true
        wait "$serve_pid" 2> /dev/null || true
        serve_pid=
        finish failed "serve didn't stop within 30 s of SIGTERM"
    fi
fi
wait "$serve_pid"
serve_code=$?
serve_pid=
window="$(( ( $(date +%s) - started ) / 60 + 3 ))m"

case "$bots_code" in
    0) ;;
    3) finish infra "the bots weren't let in (exit 3; see $log_dir/bots.log)" ;;
    *) finish failed "the bots exited $bots_code: $(grep -m1 -E 'sim (FAILED|HTTP|LOGIN|DEADLINE)' "$log_dir/bots.log" || tail -1 "$log_dir/bots.log")" ;;
esac
[ "$serve_code" = 0 ] || finish failed "serve exited $serve_code, not 0, after SIGTERM (see $log_dir/serve.log)"

# 7. The log replays, and agrees with every snapshot.
url="$url_base/$db"
"$bin/voidforge-replay" inspect postgres --database-url "$url" --json > "$log_dir/inspect.json" \
    2>> "$log_dir/replay.log" || finish failed "replay inspect failed (see $log_dir/replay.log)"
latest=$(python3 -c 'import json,sys; s=json.load(open(sys.argv[1]))["sources"][0]["snapshots"]; print(s[-1]["seq"])' "$log_dir/inspect.json")
"$bin/voidforge-replay" postgres --database-url "$url" --rules rules/v0-fast.toml --json \
    > "$log_dir/replay-earliest.json" 2>> "$log_dir/replay.log" \
    || finish failed "the log doesn't replay from its earliest snapshot (see $log_dir/replay-earliest.json)"
"$bin/voidforge-replay" postgres --database-url "$url" --rules rules/v0-fast.toml --from "$latest" --json \
    > "$log_dir/replay-latest.json" 2>> "$log_dir/replay.log" \
    || finish failed "the log doesn't replay from its latest snapshot, seq $latest (see $log_dir/replay-latest.json)"

# 8. The core's invariants after every record.
VOIDFORGE_NIGHTLY_DATABASE_URL="$url" cargo nextest run --cargo-profile release -p voidforge-host \
    --test nightly --locked --run-ignored only --no-capture -E 'test(=nightly_invariants)' \
    > "$log_dir/invariants.log" 2>&1 || finish failed "an invariant broke (see $log_dir/invariants.log)"

# 9. Step latency, from serve's own histograms.
bounds=()
[ -n "${SIM_STEP_P99_MAX:-}" ] && bounds+=(--step-p99-max "$SIM_STEP_P99_MAX")
[ -n "${SIM_APPEND_P99_MAX:-}" ] && bounds+=(--append-p99-max "$SIM_APPEND_P99_MAX")
python3 "$here/nightly-latency.py" "$run" "$window" "${bounds[@]}" > "$log_dir/latency.log" 2>&1
case $? in
    0) ;;
    3) finish infra "the collector's Prometheus went away (see $log_dir/latency.log)" ;;
    *) finish failed "step latency (see $log_dir/latency.log)" ;;
esac
finish passed "the bots played $minutes min; the log replays from its snapshots; every invariant held"
