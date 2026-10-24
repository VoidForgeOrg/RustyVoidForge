#!/usr/bin/env bash
# The nightly bot simulation (Technical Plan P5.3; decision log D-066). Against the stack's
# Postgres, dex (with the password grant) and collector:
#   1. checks dex grants bot1 a token, Postgres answers, and the collector's Prometheus answers;
#   2. builds serve, the bots and the replay tool (release, against .sqlx/, never a database);
#   3. creates this run's scratch database, voidforge_nightly_<time>_<pid> (only that prefix; a
#      leftover is dropped only when no session on the server holds it or uses it), and seeds a
#      universe for eight players on the fast rules;
#   4. starts serve on it (exporting to the collector as service.instance.id=<run>);
#   5. runs `voidforge-bots sim` for SIM_MINUTES (default 30: about 10.7 v0 weeks);
#   6. stops serve with one SIGTERM, which must exit 0;
#   7. verifies the log with `voidforge-replay postgres` from the earliest snapshot (every later
#      snapshot must equal the replay up to it) and from the last before serve's shutdown snapshot;
#   8. checks the core's invariants after every record (nightly_invariants);
#   9. reads the step and append latency through Grafana's Prometheus (nightly-latency.py), bounded
#      by SIM_STEP_P99_MAX and SIM_APPEND_P99_MAX in seconds when set;
#  10. plans and times the keyed-submission lookup on the final log (D-037; reported);
#  11. drops the database and writes the summary.
# It never touches voidforge or any database but its own.
#
# Environment: SIM_MINUTES (30), SIM_SEED (default: the time), E2E_ISSUER (default
# http://localhost:5556/dex; it needs the password grant, so locally a throwaway CI-config dex),
# OTEL_EXPORTER_OTLP_ENDPOINT (default http://localhost:4318), NIGHTLY_PROMETHEUS (see
# nightly-latency.py), NIGHTLY_LOG_DIR (default target/nightly/sim), GITHUB_STEP_SUMMARY;
# NIGHTLY_PSQL (a psql command for the server, instead of the stack's), NIGHTLY_SWEEP_ONLY.
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
# Usage errors first, before the EXIT trap: they print no summary.
case "$db" in "$prefix"[0-9]*_[0-9]*) ;; *) say "refusing database $db"; exit 2 ;; esac
case "$minutes" in ''|*[!0-9]*) say "SIM_MINUTES must be whole minutes"; exit 2 ;; esac
# The workflow's sim job has 200 minutes: 150 to play leaves ~50 over the build and checks.
[ "$minutes" -ge 1 ] && [ "$minutes" -le 150 ] || { say "SIM_MINUTES must be 1 to 150"; exit 2; }
mkdir -p "$log_dir"
# Every file a run writes, so a failed night never reports an earlier run's numbers.
rm -f "$log_dir"/*.log "$log_dir"/*.json "$log_dir"/*.out
outcome=failed
why=""

summary() {
    if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then
        # Into the job log too: a step summary has no API to read it back from.
        python3 "$here/nightly-summary.py" sim "$outcome" "$log_dir" "$why" | tee -a "$GITHUB_STEP_SUMMARY"
    else
        python3 "$here/nightly-summary.py" sim "$outcome" "$log_dir" "$why"
    fi
}

serve_pid=
created=0
holder_pid=
# A session that holds a nightly database's name as its application_name, or is connected to it.
held() { sql "SELECT count(*) FROM pg_stat_activity WHERE application_name = '$1' OR datname = '$1'"; }
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
    if [ -n "$holder_pid" ]; then
        sql "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE application_name = '$db'" > /dev/null 2>&1 || true
        kill "$holder_pid" 2> /dev/null || true
    fi
    [ -n "${NIGHTLY_SWEEP_ONLY:-}" ] || summary
    exit "$code"
}
trap cleanup EXIT
finish() { outcome=$1; why=$2; say "$outcome: $why"; case "$outcome" in passed) exit 0 ;; infra) exit 3 ;; *) exit 1 ;; esac; }


# 1. The stack.
sql "SELECT 1" > /dev/null 2>&1 || finish infra "can't reach the stack's Postgres"
# This run's claim on the shared server, before its database exists and for its whole length, the
# connection-free phases too (after CREATE, while the invariants build): a session named after the
# database, ended on exit. Another seat's sweep reads liveness here, never from ps: seats run in
# separate PID namespaces (R2-B1, as PR A's R2-2). A run killed outright leaves it sleeping for
# SIM_MINUTES + 60 minutes at most; until then its leftover is kept, late but never early.
"${psql[@]:0:${#psql[@]}-1}" "dbname=postgres application_name=$db" \
    -c "SELECT pg_sleep($(( (minutes + 60) * 60 )))" < /dev/null > /dev/null 2>&1 &
holder_pid=$!
for _ in $(seq 1 60); do [ "$(held "$db")" != 0 ] && break; sleep 0.5; done
[ "$(held "$db")" != 0 ] || finish infra "this run's holder session never appeared on the server"
say "claimed $db on the server (a session named after it, held until this run ends)"
# Leftovers of runs that never reached their EXIT trap: drop those no session holds or uses (DROP
# without FORCE refuses one that gains a connection meanwhile), list those held, and leave any
# other name alone. Never this run's own.
for old in $(sql "SELECT datname FROM pg_database WHERE datname LIKE 'voidforge\_nightly\_%' ORDER BY 1"); do
    [ "$old" = "$db" ] && continue
    case "$old" in "$prefix"[0-9]*_[0-9]*) ;; *) say "leftover $old has an unexpected name; not touching it"; continue ;; esac
    if [ "$(held "$old")" != 0 ]; then
        say "leftover $old is held by a live run's session; if that run is gone, drop it with: ${psql[*]} -c 'DROP DATABASE \"$old\"'"
    elif out=$(sql "DROP DATABASE IF EXISTS \"$old\"" 2>&1); then
        say "dropped leftover $old (no session holds it)"
    else
        say "leftover $old couldn't be dropped (in use?): ${out//$'\n'/ }"
    fi
done
# NIGHTLY_SWEEP_ONLY: the claim and the sweep, then stop (test-nightly-sweep.sh, in the database job).
[ -n "${NIGHTLY_SWEEP_ONLY:-}" ] && exit 0
grant=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$issuer/token" \
    -d 'grant_type=password&username=bot1%40voidforge.test&password=bot1-local-test-password&client_id=voidforge-cli&scope=openid' \
    || true)
[ "$grant" = 200 ] || finish infra "dex at $issuer doesn't grant bot1 a token (HTTP $grant): it needs deploy/dex/config.ci.yaml (locally, a throwaway dex on its own port: E2E_ISSUER)"
curl -sf -o /dev/null "$prometheus/api/v1/query?query=up" || finish infra "the collector's Prometheus isn't reachable at $prometheus"

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
# "The latest" is the last snapshot before serve's shutdown one, which sits at the head: from that
# one, nothing would replay.
latest=$(python3 -c 'import json,sys; s=json.load(open(sys.argv[1]))["sources"][0]["snapshots"]; print(s[-2]["seq"] if len(s) > 1 else "")' "$log_dir/inspect.json")
[ -n "$latest" ] || finish failed "the log has only one snapshot (see $log_dir/inspect.json): serve's periodic snapshots never ran"
"$bin/voidforge-replay" postgres --database-url "$url" --rules rules/v0-fast.toml --json \
    > "$log_dir/replay-earliest.json" 2>> "$log_dir/replay.log" \
    || finish failed "the log doesn't replay from its earliest snapshot (see $log_dir/replay-earliest.json)"
"$bin/voidforge-replay" postgres --database-url "$url" --rules rules/v0-fast.toml --from "$latest" --json \
    > "$log_dir/replay-latest.json" 2>> "$log_dir/replay.log" \
    || finish failed "the log doesn't replay from its latest snapshot, seq $latest (see $log_dir/replay-latest.json)"
# It started where it was told and replayed something: from the head, nothing would replay.
python3 -c '
import json, sys
s = json.load(open(sys.argv[1]))["sources"][0]
sys.exit(0 if s["records"] > 0 and s["last_seq"] - s["records"] == int(sys.argv[2]) else 1)' \
    "$log_dir/replay-latest.json" "$latest" \
    || finish failed "the replay from seq $latest didn't start there, or replayed no records (see $log_dir/replay-latest.json)"

# 8. The core's invariants after every record.
VOIDFORGE_NIGHTLY_DATABASE_URL="$url" cargo nextest run --cargo-profile release -p voidforge-host \
    --test nightly --locked --run-ignored only --no-capture -E 'test(=nightly_invariants)' \
    > "$log_dir/invariants.log" 2>&1 || finish failed "an invariant broke (see $log_dir/invariants.log)"
# Every state was checked: the snapshot's, then one after each record.
python3 -c '
import json, sys
line = next(l for l in open(sys.argv[1]) if l.startswith("@nightly-invariants "))
r = json.loads(line.split(" ", 1)[1])
sys.exit(0 if r["checked"] == r["records"] + 1 else 1)' "$log_dir/invariants.log" \
    || finish failed "the invariants weren't checked after every record (see $log_dir/invariants.log)"

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
# 10. D-037: the keyed-submission lookup (find_keys' query, voidforge-store's log.rs), planned and
# timed on the log at its final size, for the last key stored. Reported, not bounded.
last="(SELECT %s FROM log_records WHERE idempotency_key IS NOT NULL ORDER BY seq DESC LIMIT 1)"
# shellcheck disable=SC2059
explain="EXPLAIN (ANALYZE, BUFFERS) SELECT seq FROM log_records WHERE (player_id, idempotency_key) IN
    (SELECT p::uuid, k FROM unnest(ARRAY[$(printf "$last" player_id::text)], ARRAY[$(printf "$last" idempotency_key)]) AS u(p, k))
    ORDER BY seq"
"${psql[@]:0:${#psql[@]}-1}" "$db" -c "$explain" < /dev/null > "$log_dir/keys-explain.log" 2>&1 \
    || say "the key lookup's EXPLAIN failed (see $log_dir/keys-explain.log): reported, not a verdict"
finish passed "the bots played $minutes min; the log replays from its snapshots; every invariant held"
