#!/usr/bin/env bash
# Both images under compose, healthy (Technical Plan P5.2; decision log D-065, the advisor's A1):
# what the Phase 5 gate means by "run under compose". Run by the CI images job and
# `just images-compose`, after `just images` and with the stack's Postgres and a dex up.
#
#   1. creates its own database, voidforge_e2e_<time>_<pid>, and seeds it from the server image;
#   2. brings up deploy/compose.images.yaml as its own project, voidforge-images, with
#      `--wait`: compose returns only once both healthchecks pass (serve's GET /openapi.json, the
#      web's GET /), and the script checks each container's health is "healthy" and that it runs
#      the image ID it was given;
#   3. `docker compose stop`: SIGTERM to each PID 1, then each must exit 0 (serve with its final
#      snapshot at the log head, D-050);
#   4. takes the project down and drops its database, on every exit.
#
# Owner safety: the only database it creates, drops or names is its own voidforge_e2e_*; it never
# touches voidforge, and the project is never the stack's `voidforge` (check-images.py --static).
# E2E_ISSUER (default http://localhost:5556/dex) names the dex; the web needs only its discovery.
# Ports: serve on a free port, the web on 127.0.0.1:8081 (dex registers that redirect), which must
# be free. Logs go to E2E_LOG_DIR (default target/e2e/images-<run>).
# Exit codes: 0 both healthy and both stopped cleanly; 1 a check failed; 3 the stack isn't ready.
set -euo pipefail

issuer="${E2E_ISSUER:-http://localhost:5556/dex}"
prefix=voidforge_e2e_
run="$(date -u +%Y%m%d%H%M%S)_$$"
db="${prefix}${run}"
url_base="postgres://voidforge:voidforge-local-test-password@localhost:5432"
log_dir="${E2E_LOG_DIR:-target/e2e/images-$run}"
psql=(docker compose -f deploy/compose.yaml exec -T postgres psql -U voidforge -X -q -v ON_ERROR_STOP=1 -At -d postgres)
sql() { "${psql[@]}" -c "$1" < /dev/null; }
say() { echo "images-compose: $*" >&2; }
images=(docker compose -f deploy/compose.images.yaml)

case "$db" in
    "$prefix"[0-9]*_[0-9]*) ;;
    *) say "refusing database $db"; exit 1 ;;
esac
mkdir -p "$log_dir"
for image in voidforge-server:local voidforge-web:local; do
    docker image inspect "$image" > /dev/null 2>&1 || { say "the image $image isn't built; build both with: just images"; exit 3; }
done
server_image=$(docker image inspect -f '{{.Id}}' voidforge-server:local)
web_image=$(docker image inspect -f '{{.Id}}' voidforge-web:local)
sql "SELECT 1" > /dev/null || { say "can't reach the stack's Postgres; start it with: just up"; exit 3; }
curl -sf -o /dev/null "$issuer/.well-known/openid-configuration" || { say "no dex at $issuer"; exit 3; }

port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
export VOIDFORGE_DATABASE_URL="$url_base/$db" VOIDFORGE_PORT="$port" VOIDFORGE_ISSUER="$issuer" \
    VOIDFORGE_RULES_FILE=v0-fast.toml VOIDFORGE_WEB_CLIENT_SECRET=voidforge-web-local-test-secret
up=0
cleanup() {
    local code=$?
    if [ "$up" = 1 ]; then
        "${images[@]}" logs --no-color > "$log_dir/compose.log" 2>&1 || true
        "${images[@]}" down --timeout 5 --remove-orphans > /dev/null 2>&1 \
            || say "FAILED to take the project down; run: docker compose -p voidforge-images down"
    fi
    if out=$(sql "DROP DATABASE IF EXISTS \"$db\"" 2>&1); then
        say "dropped $db; logs in $log_dir"
    else
        say "FAILED to drop $db: ${out//$'\n'/ }"
        [ "$code" != 0 ] || code=1
    fi
    exit "$code"
}
trap cleanup EXIT

# 1. Its own universe, seeded from the server image.
sql "CREATE DATABASE \"$db\""
docker run --rm --network host -e VOIDFORGE_DATABASE_URL "$server_image" \
    seed --seed 20261020 --players 2 --rules /etc/voidforge/rules/v0-fast.toml > "$log_dir/seed.log" 2>&1 \
    || { say "seed failed:"; cat "$log_dir/seed.log" >&2; exit 1; }

# 2. Both up, health-gated.
up=1
started=$(date +%s)
if ! "${images[@]}" up --no-build --detach --wait --wait-timeout 360 > "$log_dir/up.log" 2>&1; then
    say "FAILED: compose didn't bring both images up healthy:"
    cat "$log_dir/up.log" >&2
    "${images[@]}" logs --no-color >&2 || true
    exit 1
fi
say "both up and healthy in $(( $(date +%s) - started )) s"
failed=0
for service in server web; do
    container=$("${images[@]}" ps -q "$service")
    health=$(docker inspect -f '{{.State.Health.Status}}' "$container")
    ran=$(docker inspect -f '{{.Image}}' "$container")
    want=$server_image
    [ "$service" = web ] && want=$web_image
    say "$service: container ${container:0:12}, image $ran, health $health"
    [ "$health" = healthy ] || { say "FAILED: $service is $health, not healthy"; failed=1; }
    [ "$ran" = "$want" ] || { say "FAILED: $service runs $ran, not $want"; failed=1; }
done
[ "$failed" = 0 ] || exit 1

# 3. One SIGTERM each, exit 0 each.
containers=$("${images[@]}" ps -q)
"${images[@]}" stop > /dev/null 2>&1
for container in $containers; do
    name=$(docker inspect -f '{{index .Config.Labels "com.docker.compose.service"}}' "$container")
    code=$(docker inspect -f '{{.State.ExitCode}}' "$container")
    say "$name stopped with exit $code"
    [ "$code" = 0 ] || { say "FAILED: $name exited $code after docker compose stop"; failed=1; }
done
"${images[@]}" logs --no-color --no-log-prefix server > "$log_dir/serve.log" 2>&1 || true
if ! grep -q 'stopped cleanly; the final snapshot is at the log head' "$log_dir/serve.log"; then
    say "FAILED: serve's log doesn't say its final snapshot is at the log head"
    failed=1
fi
[ "$failed" = 0 ] || exit 1
say "PASSED: both images ran under compose (project voidforge-images), healthy, and stopped cleanly"
