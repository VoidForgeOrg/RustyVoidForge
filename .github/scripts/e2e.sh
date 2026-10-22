#!/usr/bin/env bash
# The end-to-end scenario (Technical Plan P3.6; decision log D-052), run by the CI e2e job and by
# `just e2e`, against the compose stack's Postgres and dex (the CI dex config: password grant on).
# With --smoke (`just smoke`, P4.5, D-062) it runs only the web walk in step 5: the smoke test is
# the same walk CI runs inside the whole scenario, never a second copy.
#
#   1. checks dex's password grant, and lists leftover e2e databases;
#   2. builds voidforge-server, voidforge-bots and voidforge-web;
#   3. creates this run's own database, voidforge_e2e_<time>_<pid>, and seeds it on the fast rules;
#   4. starts the serve binary as a host process (never through just or cargo run: D-050 I5) on a
#      free port, and waits for its ready line and GET /openapi.json;
#   5. runs `voidforge-bots e2e`, then the terminal login check (.github/scripts/device-login.py),
#      then the web login check (.github/scripts/web-login.py; it needs localhost:8081 free, and
#      refuses to start if anything already listens there);
#   6. stops serve with one SIGTERM and requires exit 0 (the final snapshot at the log head);
#   7. drops this run's database, on every exit.
#
# Owner safety: the only databases this script creates, drops or names are voidforge_e2e_*. It
# never touches voidforge, voidforge_dev or voidforge_test, and it never reconfigures dex. A
# leftover from a killed run is dropped only when the pid in its name is dead; otherwise it's
# listed with the command that drops it.
#
# With --images (P5.2, D-065; `just smoke-images` with --smoke) the server and the web app are the
# images, voidforge-server:local and voidforge-web:local (`just images`), not cargo builds: seed runs
# from the server image, serve runs under compose (deploy/compose.images.yaml, as this run's own
# project voidforge-images-e2e-<pid>), and the web walk starts the web image as its own child with
# `docker run --rm -i` by image ID. Step 6 is then `docker compose stop`, which must end in exit 0.
# Both images' IDs are printed, and the serve container is checked to run the server image's ID.
#
# E2E_ISSUER (default http://localhost:5556/dex) names the dex to use. Logs go to E2E_LOG_DIR (default target/e2e/<run>): serve.log, bots.log, seed.log,
# device-login.log, web-login.log and web.log.
# Usage: e2e.sh [--smoke] [--images]
# Exit codes: 0 passed; 1 the scenario failed, or serve didn't stop cleanly; 2 usage;
# 3 the stack isn't ready (dex without the password grant, Postgres unreachable, serve not ready).
set -euo pipefail

seed=20261020
players=2
rules=rules/v0-fast.toml
# E2E_ISSUER points at another dex with the CI config, for example a throwaway one on another
# port, so a run needn't touch the stack's own dex.
issuer="${E2E_ISSUER:-http://localhost:5556/dex}"
prefix=voidforge_e2e_
run="$(date -u +%Y%m%d%H%M%S)_$$"
db="${prefix}${run}"
url_base="postgres://voidforge:voidforge-local-test-password@localhost:5432"
log_dir="${E2E_LOG_DIR:-target/e2e/$run}"
psql=(docker compose -f deploy/compose.yaml exec -T postgres psql -U voidforge -X -q -v ON_ERROR_STOP=1 -At -d postgres)
sql() { "${psql[@]}" -c "$1" < /dev/null; }
say() { echo "e2e: $*" >&2; }
smoke=0
images=0
for arg in "$@"; do
    case "$arg" in
        --smoke) smoke=1 ;;
        --images) images=1 ;;
        *) say "usage: e2e.sh [--smoke] [--images]"; exit 2 ;;
    esac
done
# The images' compose project for this run: never the owner's `voidforge` (D-065).
images_compose=(docker compose -f deploy/compose.images.yaml -p "voidforge-images-e2e-$$")
web_secret=voidforge-web-local-test-secret  # TEST-ONLY, from deploy/dex/config*.yaml
drop_hint() { say "  docker compose -f deploy/compose.yaml exec -T postgres psql -U voidforge -d postgres -c 'DROP DATABASE \"$1\"'"; }

case "$db" in
    "$prefix"[0-9]*_[0-9]*) ;;
    *) say "refusing database $db"; exit 2 ;;
esac
mkdir -p "$log_dir"
# A reused E2E_LOG_DIR may hold an earlier run's walk log, whose timing line would satisfy the
# walk-ran check below without this run walking at all (P4.5 QA's F1).
rm -f "$log_dir/web-login.log"

# 1. The stack: dex's password grant for the static users, as the bots' public client.
grant=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$issuer/token" \
    -d 'grant_type=password&username=alice%40voidforge.test&password=alice-local-test-password&client_id=voidforge-cli&scope=openid' \
    || true)
if [ "$grant" != 200 ]; then
    say "dex at $issuer doesn't grant alice a token through the password grant (HTTP $grant)."
    say "Point E2E_ISSUER at a throwaway dex running deploy/dex/config.ci.yaml (its issuer on its own"
    say "port), for example E2E_ISSUER=http://localhost:5563/dex just smoke. Or, if the stack's dex is"
    say "yours to reconfigure, start the stack with the CI dex config: DEX_CONFIG=config.ci.yaml just up"
    exit 3
fi
if ! sql "SELECT 1" > /dev/null; then
    say "can't reach the stack's Postgres; start it with: just up"
    exit 3
fi
# Leftovers: drop those whose creating pid is dead; list the rest. `ps -p` sees every user's
# processes; `kill -0` fails with EPERM for another user's, which would read as dead.
for old in $(sql "SELECT datname FROM pg_database WHERE datname LIKE 'voidforge\_e2e\_%' ORDER BY 1"); do
    case "$old" in "$prefix"[0-9]*_[0-9]*) ;; *) say "leftover $old has an unexpected name; not touching it"; continue ;; esac
    pid="${old##*_}"
    if ps -p "$pid" > /dev/null 2>&1; then
        say "leftover $old: pid $pid is alive (perhaps another run); drop it yourself if it isn't:"
        drop_hint "$old"
    elif out=$(sql "DROP DATABASE IF EXISTS \"$old\"" 2>&1); then
        say "dropped leftover $old (pid $pid is gone)"
    else
        # Its pid is gone, but something still holds it (a serve left running, say): report it
        # and go on, since this run's own database is separate.
        say "leftover $old: pid $pid is gone, but it couldn't be dropped: ${out//$'\n'/ }"
        say "  stop whatever holds it, then:"
        drop_hint "$old"
    fi
done

# 2. The binaries, checked against .sqlx/ (D-034), never against a database; with --images, the
# images instead of the server and the web app (the bots still build, for the whole scenario).
# `--images --smoke` builds nothing with cargo, so it needs no toolchain (the CI images job).
if [ "$images" = 0 ] || [ "$smoke" = 0 ]; then
    target_dir=$(cargo metadata -q --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
    server="$target_dir/debug/voidforge-server"
    bots="$target_dir/debug/voidforge-bots"
    web="$target_dir/debug/voidforge-web"
fi
if [ "$images" = 1 ]; then
    for image in voidforge-server:local voidforge-web:local; do
        if ! docker image inspect "$image" > /dev/null 2>&1; then
            say "the image $image isn't built; build both with: just images"
            exit 3
        fi
    done
    server_image=$(docker image inspect -f '{{.Id}}' voidforge-server:local)
    web_image=$(docker image inspect -f '{{.Id}}' voidforge-web:local)
    say "images: voidforge-server:local $server_image, voidforge-web:local $web_image"
    [ "$smoke" = 1 ] || SQLX_OFFLINE=true cargo build -q --locked -p voidforge-bots
else
    SQLX_OFFLINE=true cargo build -q --locked -p voidforge-server -p voidforge-bots -p voidforge-web
fi

serve_pid=
serve_container=
cleanup() {
    local code=$?
    if [ -n "$serve_pid" ] && kill -0 "$serve_pid" 2> /dev/null; then
        kill -KILL "$serve_pid" 2> /dev/null || true
        wait "$serve_pid" 2> /dev/null || true
    fi
    # The walk's web container, if a crash left it running (it holds :8081).
    if [ "$images" = 1 ] && docker container inspect "voidforge-walk-web-$$" > /dev/null 2>&1; then
        docker rm -f "voidforge-walk-web-$$" > /dev/null 2>&1 \
            || say "FAILED to remove the walk's web container; remove it with: docker rm -f voidforge-walk-web-$$"
    fi
    # The images' project goes whole, its serve container first killed if it still runs.
    if [ -n "$serve_container" ]; then
        "${images_compose[@]}" logs --no-color --no-log-prefix server > "$log_dir/serve.log" 2>&1 || true
        "${images_compose[@]}" kill server > /dev/null 2>&1 || true
        "${images_compose[@]}" down --remove-orphans > /dev/null 2>&1 \
            || say "FAILED to remove the compose project voidforge-images-e2e-$$; remove it with: docker compose -p voidforge-images-e2e-$$ down"
    fi
    # serve is gone, so nothing holds the database: no FORCE. A failed drop is reported, and fails
    # the run: it leaves a database behind.
    if out=$(sql "DROP DATABASE IF EXISTS \"$db\"" 2>&1); then
        say "dropped $db; logs in $log_dir"
    else
        say "FAILED to drop $db: ${out//$'\n'/ }; logs in $log_dir. Drop it by hand:"
        drop_hint "$db"
        [ "$code" != 0 ] || code=1
    fi
    exit "$code"
}
trap cleanup EXIT

# 3. A fresh universe.
sql "CREATE DATABASE \"$db\""
say "created $db"
if [ "$images" = 1 ]; then
    seed_cmd=(docker run --rm --network host -e VOIDFORGE_DATABASE_URL "$server_image")
    seed_rules="/etc/voidforge/$rules"
else
    seed_cmd=("$server")
    seed_rules="$rules"
fi
VOIDFORGE_DATABASE_URL="$url_base/$db" "${seed_cmd[@]}" seed --seed "$seed" --players "$players" --rules "$seed_rules" \
    > "$log_dir/seed.log" 2>&1 || { say "seed failed:"; cat "$log_dir/seed.log" >&2; exit 1; }

# 4. serve, on a free port, with the board rebuilt every second: a host process, or with --images
# the server image under compose (host networking, so its port is this host's).
serve_alive() {
    if [ "$images" = 1 ]; then
        [ "$(docker inspect -f '{{.State.Running}}' "$serve_container" 2> /dev/null)" = true ]
    else
        kill -0 "$serve_pid" 2> /dev/null
    fi
}
if [ "$images" = 1 ]; then
    port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
    # Exported, since compose reads every variable of the file on each command, cleanup's too.
    export VOIDFORGE_DATABASE_URL="$url_base/$db" VOIDFORGE_PORT="$port" VOIDFORGE_ISSUER="$issuer" \
        VOIDFORGE_RULES_FILE="${rules#rules/}" VOIDFORGE_LEADERBOARD_SECS=1 VOIDFORGE_WEB_CLIENT_SECRET="$web_secret"
    "${images_compose[@]}" up --detach --no-build server > "$log_dir/compose-up.log" 2>&1 \
        || { say "compose couldn't start serve:"; cat "$log_dir/compose-up.log" >&2; exit 3; }
    serve_container=$("${images_compose[@]}" ps -aq server)
    ran=$(docker inspect -f '{{.Image}}' "$serve_container")
    if [ "$ran" != "$server_image" ]; then
        say "FAILED: the serve container runs $ran, not voidforge-server:local ($server_image)"
        exit 1
    fi
fi
addr=
for _ in $(seq 1 300); do
    if [ "$images" = 1 ]; then
        "${images_compose[@]}" logs --no-color --no-log-prefix server > "$log_dir/serve.log" 2>&1 || true
    fi
    addr=$(sed -n 's/^ready: listening on \([^,]*\),.*/\1/p' "$log_dir/serve.log")
    [ -n "$addr" ] && break
    serve_alive || break
    sleep 0.2
done
# With --images on macOS the ready line names the bind in colima's VM; the Mac's side of the port is
# colima's forward, ~20-30 ms later (D-065), so the first GET may retry for up to 5 s.
openapi_ok() {
    local tries=1
    [ "$images" = 1 ] && tries=250
    for _ in $(seq 1 "$tries"); do
        curl -sf -o /dev/null "http://$addr/openapi.json" && return 0
        sleep 0.02
    done
    return 1
}
if [ -z "$addr" ] || ! openapi_ok; then
    say "serve isn't ready; its log:"
    cat "$log_dir/serve.log" >&2
    exit 3
fi
if [ "$images" = 1 ]; then
    say "serve is ready on $addr (container ${serve_container:0:12}, image $server_image)"
else
    say "serve is ready on $addr (pid $serve_pid)"
fi

# 5. The bots (skipped by --smoke).
set +e
if [ "$smoke" = 1 ]; then
    say "smoke: the bots' scenario and the terminal login are skipped; the web walk runs alone"
    bots_code=0
else
    "$bots" e2e --api "http://$addr" --issuer "$issuer" --run "$run" 2>&1 | tee "$log_dir/bots.log"
    bots_code=${PIPESTATUS[0]}
fi
# 5b. The terminal login (P3.7, D-053): bob logs in through dex's device code flow, approved by
# driving dex's own pages, then plays a little with the saved login. It runs after the scenario,
# so it can't disturb it.
device_code=0
if [ "$bots_code" = 0 ] && [ "$smoke" = 0 ]; then
    python3 .github/scripts/device-login.py --bots "$bots" --api "http://$addr" --issuer "$issuer" \
        2>&1 | tee "$log_dir/device-login.log"
    device_code=${PIPESTATUS[0]}
fi
# 5c. The web login (P4.1, D-057): alice logs in and out of voidforge-web through dex's
# authorization code flow, driven over HTTP with cookie jars; Home's score proves the API took
# the web app's token, and the page walk (P4.3, D-060) reads every page from the live game. It
# starts and stops voidforge-web itself, on localhost:8081.
# With --images, the walk starts the web image by ID through `docker run --rm -i`: the one form
# that relays its stop's SIGTERM and returns the container's exit code (P5.2's spikes, D-065).
web_code=0
if [ "$images" = 1 ]; then
    # Named per run, so cleanup can remove one a crash left behind.
    web_args=(--web-cmd "docker run --rm -i --name voidforge-walk-web-$$ --network host -e VOIDFORGE_WEB_CLIENT_SECRET $web_image")
else
    web_args=(--web "$web")
fi
if [ "$bots_code" = 0 ]; then
    python3 .github/scripts/web-login.py "${web_args[@]}" --api "http://$addr" --issuer "$issuer" \
        --log "$log_dir/web.log" 2>&1 | tee "$log_dir/web-login.log"
    web_code=${PIPESTATUS[0]}
fi
set -e

# 6. One SIGTERM: serve drains and stops with its final snapshot, exit 0 (D-050). A serve that
# already exited during the scenario is reported as such, never as a stop. With --images it's
# `docker compose stop`: SIGTERM to serve as PID 1, SIGKILL only after the grace period (40 s).
stopped_by="one SIGTERM"
if [ "$images" = 1 ]; then
    if serve_alive; then
        stopped_by="docker compose stop (one SIGTERM)"
        stop_started=$(date +%s)
        "${images_compose[@]}" stop server > /dev/null 2>&1 || true
        say "docker compose stop took $(( $(date +%s) - stop_started )) s"
    else
        stopped_by="nothing: it had already exited during the scenario"
    fi
    "${images_compose[@]}" logs --no-color --no-log-prefix server > "$log_dir/serve.log" 2>&1 || true
    serve_code=$(docker inspect -f '{{.State.ExitCode}}' "$serve_container")
elif kill -0 "$serve_pid" 2> /dev/null; then
    kill -TERM "$serve_pid"
else
    stopped_by="nothing: it had already exited during the scenario"
fi
if [ "$images" = 0 ]; then
    for _ in $(seq 1 100); do
        kill -0 "$serve_pid" 2> /dev/null || break
        sleep 0.2
    done
    if kill -0 "$serve_pid" 2> /dev/null; then
        say "serve didn't stop within 20 s of SIGTERM; killing it"
        kill -KILL "$serve_pid" 2> /dev/null || true
        wait "$serve_pid" 2> /dev/null || true
        serve_code=124
    else
        set +e
        wait "$serve_pid"
        serve_code=$?
        set -e
    fi
fi
# serve has been reaped on every path above, so cleanup has nothing left to kill.
serve_pid=
if [ "$bots_code" = 0 ] && [ "$device_code" != 0 ]; then
    say "FAILED: the scenario passed, but the terminal login check exited $device_code; its log is $log_dir/device-login.log"
    exit 1
fi
# The walk must have run to its end: web-login.py prints its timing line only once steps 1 to 9
# (login, pages, the forms' MVP loop, replay, logout) have all passed (P4.5, D-062), so a run that
# skipped it, or stopped short with exit 0, can't pass.
if [ "$bots_code" = 0 ] && [ "$web_code" = 0 ] \
    && ! grep -q '^ok: the smoke walk took ' "$log_dir/web-login.log" 2> /dev/null; then
    say "FAILED: the web walk didn't run to its end (no timing line in $log_dir/web-login.log)"
    exit 1
fi
if [ "$bots_code" = 0 ] && [ "$web_code" != 0 ]; then
    if [ "$smoke" = 1 ]; then
        say "FAILED (smoke): the web walk exited $web_code; its log is $log_dir/web-login.log, the web app's $log_dir/web.log"
        exit 1
    fi
    say "FAILED: the scenario passed, but the web login check exited $web_code; its log is $log_dir/web-login.log, the web app's $log_dir/web.log"
    exit 1
fi
if [ "$bots_code" != 0 ]; then
    say "FAILED: the bots exited $bots_code; serve exited $serve_code after $stopped_by; serve's log is $log_dir/serve.log"
    exit 1
fi
if [ "$serve_code" != 0 ]; then
    say "FAILED: the scenario passed, but serve exited $serve_code after $stopped_by; its log:"
    tail -n 40 "$log_dir/serve.log" >&2
    exit 1
fi
if [ "$smoke" = 1 ]; then
    say "PASSED (smoke): the web walk passed, and serve stopped cleanly; $(grep '^ok: the smoke walk took ' "$log_dir/web-login.log")"
else
    say "PASSED: the scenario, the terminal login and the web login passed, and serve stopped cleanly"
fi
