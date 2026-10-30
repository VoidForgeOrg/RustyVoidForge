#!/usr/bin/env bash
# Regression cases for the nightly's readiness waits (slice 53, D-066; night 7, issue #65), with no
# stack: Postgres is a fake psql, dex, Grafana's Prometheus and serve are local HTTP servers that
# answer 503 until some seconds after their first request (night 7's proxy), and cargo, serve and
# the bots are fakes, so nothing reaches a real server or database.
#   - every readiness probe in nightly-sim.sh (Postgres, the run's holder session, dex's grant,
#     Prometheus, serve) waits for a late service, and the run goes on to the bots, whose fake
#     stops it;
#   - each one, when its service never answers, ends infra (exit 3) at its bound, saying how long
#     it waited; serve exiting before ready ends its wait at once;
#   - nightly-crash.sh's Postgres probe waits the same way, and ends infra at its bound.
# A wait that's removed fails a "late" case; a bound that's lost hangs a "never" case into its
# timeout.
#
# Usage: test-nightly-ready.sh (from the repository root, or anywhere)
# Exit codes: 0 every case passed; 1 one failed.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/../.."
tmp=$(mktemp -d)
pids=()
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2> /dev/null; done
    rm -rf "$tmp"
}
trap cleanup EXIT
bad=0
check() {
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "FAIL: $1: expected '$3', found '$2'"; bad=1; fi
}

mkdir -p "$tmp/bin" "$tmp/target/release"
# An HTTP server that answers 503 to a path until <late> seconds after that path's first request.
cat > "$tmp/late.py" <<'EOF'
import sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
port, late = int(sys.argv[1]), float(sys.argv[2])
first = {}
class Late(BaseHTTPRequestHandler):
    def answer(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        t = first.setdefault(self.path.split("?")[0], time.monotonic())
        body = b'{"status":"success","data":{"resultType":"vector","result":[]}}'
        self.send_response(200 if time.monotonic() - t >= late else 503)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = answer
    def log_message(self, *a):
        pass
HTTPServer(("127.0.0.1", port), Late).serve_forever()
EOF
# psql: refuses until FAKE_PG_LATE seconds after the first attempt; the holder session shows on
# the server FAKE_HOLD_LATE seconds after it starts.
cat > "$tmp/bin/psql" <<'EOF'
#!/usr/bin/env bash
q=
while [ $# -gt 0 ]; do [ "$1" = -c ] && q=$2; shift; done
since() { [ -f "$FAKE_DIR/$1" ] || date +%s > "$FAKE_DIR/$1"; echo $(( $(date +%s) - $(cat "$FAKE_DIR/$1") )); }
[ "$(since pg)" -ge "$FAKE_PG_LATE" ] || { echo "psql: connection refused (fake)" >&2; exit 2; }
case "$q" in
    *pg_sleep*) date +%s > "$FAKE_DIR/holder"; exec sleep 600 ;;
    *pg_terminate_backend*) echo 0 ;;
    *"count(*) FROM pg_stat_activity"*)
        if [ -f "$FAKE_DIR/holder" ] && [ "$(since holder)" -ge "$FAKE_HOLD_LATE" ]; then echo 1; else echo 0; fi ;;
    *"SELECT datname"*) ;;
    *) echo 1 ;;
esac
EOF
cat > "$tmp/bin/cargo" <<EOF
#!/usr/bin/env bash
case "\$1" in metadata) echo '{"target_directory": "$tmp/target"}' ;; esac
exit 0
EOF
# serve: its ready line after a second, then an answer FAKE_SERVE_LATE seconds after the first
# request; FAKE_SERVE=dies exits before ready.
cat > "$tmp/target/release/voidforge-server" <<EOF
#!/usr/bin/env bash
[ "\$1" = serve ] || exit 0
[ "\${FAKE_SERVE:-}" = dies ] && { echo "error: the fake serve exits"; exit 1; }
sleep 1
echo "ready: listening on 127.0.0.1:\$FAKE_SERVE_PORT, seq 0"
exec python3 "$tmp/late.py" "\$FAKE_SERVE_PORT" "\$FAKE_SERVE_LATE"
EOF
printf '#!/usr/bin/env bash\necho "sim FAILED: the fake bots stop here" >&2\nexit 1\n' \
    > "$tmp/target/release/voidforge-bots"
cp "$tmp/target/release/voidforge-bots" "$tmp/target/release/voidforge-replay"
chmod +x "$tmp/bin/"* "$tmp/target/release/"*
# nightly-crash.sh beside a fake database.sh, which passes.
mkdir -p "$tmp/crash"
cp "$here/nightly-crash.sh" "$here/nightly-ready.sh" "$here/nightly-summary.py" "$tmp/crash/"
printf '#!/usr/bin/env bash\necho "database.sh (fake): passed"\n' > "$tmp/crash/database.sh"
chmod +x "$tmp/crash/database.sh"

port() { python3 "$here/free-port.py"; }
late_server() { python3 "$tmp/late.py" "$1" "$2" > /dev/null 2>&1 & pids+=($!); }
waited() { sed -n "s/.*waited \([0-9]*\) s.*/\1/p" "$1" | head -1; }
within() { [ -n "$1" ] && [ "$1" -ge "$2" ] && [ "$1" -le "$3" ] && echo yes || echo "no ($1)"; }

# sim <name> <pg late> <holder late> <dex late> <prometheus late> <serve late> [VAR=value...]
sim() {
    local name=$1 dex prom
    dex=$(port); prom=$(port)
    late_server "$dex" "$4"; late_server "$prom" "$5"
    mkdir -p "$tmp/$name"
    sleep 0.5
    env -u GITHUB_STEP_SUMMARY PATH="$tmp/bin:$PATH" FAKE_DIR="$tmp/$name" FAKE_PG_LATE="$2" \
        FAKE_HOLD_LATE="$3" FAKE_SERVE_PORT="$(port)" FAKE_SERVE_LATE="$6" \
        NIGHTLY_PSQL="$tmp/bin/psql" E2E_ISSUER="http://127.0.0.1:$dex" \
        NIGHTLY_PROMETHEUS="http://127.0.0.1:$prom" NIGHTLY_LOG_DIR="$tmp/$name/log" SIM_MINUTES=1 \
        "${@:7}" timeout 120 "$here/nightly-sim.sh" > "$tmp/$name.log" 2>&1
    code=$?
}

sim late 3 1 3 3 2
check "sim: every service late, the run reaches the bots (exit 1, their fake)" "$code" 1
check "sim: and says why" "$(grep -c 'nightly-sim: failed: the bots exited 1: sim FAILED' "$tmp/late.log")" 1
check "sim: past serve's wait" "$(grep -c 'nightly-sim: serve is ready on' "$tmp/late.log")" 1
[ "$code" = 1 ] || tail -5 "$tmp/late.log"

sim pg-never 1000 0 0 0 0 NIGHTLY_READY_WAIT=4
check "sim: Postgres never answers: infra" "$code" 3
check "sim: after its bound, said" "$(within "$(waited "$tmp/pg-never.log")" 4 20)" yes
check "sim: and why" "$(grep -c "nightly-sim: infra: can't reach the stack's Postgres" "$tmp/pg-never.log")" 1

sim holder-never 0 1000 0 0 0
check "sim: the holder session never shows: infra" "$code" 3
check "sim: after its 30 s, said" "$(within "$(waited "$tmp/holder-never.log")" 30 45)" yes

sim dex-never 0 0 1000 0 0 NIGHTLY_READY_WAIT=4
check "sim: dex never grants: infra" "$code" 3
check "sim: after its bound, said" "$(within "$(waited "$tmp/dex-never.log")" 4 20)" yes
check "sim: with dex's last answer" "$(grep -c "nightly-sim: infra: dex at .* doesn't grant bot1 a token (HTTP 503, waited" "$tmp/dex-never.log")" 1

sim prometheus-never 0 0 0 1000 0 NIGHTLY_READY_WAIT=4
check "sim: Prometheus never answers: infra" "$code" 3
check "sim: after its bound, said" "$(within "$(waited "$tmp/prometheus-never.log")" 4 20)" yes
check "sim: and why" "$(grep -c "nightly-sim: infra: the collector's Prometheus isn't reachable" "$tmp/prometheus-never.log")" 1

sim serve-never 0 0 0 0 1000 NIGHTLY_SERVE_WAIT=4
check "sim: serve never answers: infra" "$code" 3
check "sim: after its bound, said" "$(within "$(waited "$tmp/serve-never.log")" 4 20)" yes

sim serve-dies 0 0 0 0 0 FAKE_SERVE=dies NIGHTLY_SERVE_WAIT=60
check "sim: serve exits before ready: infra" "$code" 3
check "sim: at once, not at its bound" "$(within "$(waited "$tmp/serve-dies.log")" 0 5)" yes

# crash <name> <pg late> [VAR=value...]
crash() {
    mkdir -p "$tmp/$1"
    env -u GITHUB_STEP_SUMMARY PATH="$tmp/bin:$PATH" FAKE_DIR="$tmp/$1" FAKE_PG_LATE="$2" \
        FAKE_HOLD_LATE=0 DATABASE_URL=postgres://fake/voidforge_test PSQL="$tmp/bin/psql" \
        NIGHTLY_LOG_DIR="$tmp/$1/log" VOIDFORGE_CRASH_SEEDS=1 VOIDFORGE_CRASH_PG_SEEDS=1 \
        "${@:3}" timeout 120 "$tmp/crash/nightly-crash.sh" > "$tmp/$1.log" 2>&1
    code=$?
}
crash crash-late 3
check "crash: Postgres late, the run reaches the database stage and passes" "$code" 0
check "crash: past the probe" "$(grep -c 'database.sh (fake): passed' "$tmp/crash-late.log")" 1
[ "$code" = 0 ] || tail -5 "$tmp/crash-late.log"
crash crash-never 1000 NIGHTLY_READY_WAIT=4
check "crash: Postgres never answers: infra" "$code" 3
check "crash: after its bound, said" "$(within "$(waited "$tmp/crash-never.log")" 4 20)" yes

exit "$bad"
