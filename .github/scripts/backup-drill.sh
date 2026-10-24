#!/usr/bin/env bash
# The backup and restore drill (Technical Plan P5.4; decision log D-067; deploy/RUNBOOK.md has the
# same steps for a person). Two scratch Postgres stacks from deploy/compose.drill.yaml, each its
# own compose project, port and volume, both taken down with their volumes on every exit:
#   1. builds serve, the bots and the replay tool (one build, so the replay accepts the log);
#   2. checks dex grants bot1 a token (the bots log in by the password grant);
#   3. brings up the source and the fresh restore stack, healthy;
#   4. seeds the source (8 players, the fast rules), starts serve on it, and starts the bots;
#   5. waits, bounded, for serve's first periodic snapshot (every 1,000 records), then a tail;
#   6. backs up the source with pg_dump -Fc inside its container while serve and the bots run;
#   7. lets the bots play on past the backup, then stops them and serve (one SIGTERM, which must
#      exit 0): the source's log must have grown past the backup's;
#   8. restores into the fresh stack: pg_restore --exit-on-error --single-transaction;
#   9. B is the restored head. The replay hashes the restored log and the source's --to B: both
#      verified against every snapshot up to B, the same last seq and the same end hash; the
#      tables compare equal up to B (the log, the snapshots, the accounts, the epoch);
#  10. starts serve on the restored database (it must recover to seq B), lets the bots play a
#      minute on it, stops it, and replays the restored log again: verified, with commands past B;
#  11. the negatives, each through the same check as the clean run: a truncated dump must fail
#      pg_restore; a restored log with one record tampered must fail the replay and the tables'
#      comparison; a restored copy without its oldest snapshot must fail the tables' comparison, and
#      without its epoch serve must exit before ready; the source's hash halfway to B must differ
#      from B's; its URL check must refuse port 5432; and a backup taken before any periodic
#      snapshot must fail the snapshot check. If one passes, the drill fails.
# It never names the stack's project, containers, port 5432 or the voidforge database.
#
# Environment: E2E_ISSUER (default http://localhost:5556/dex; it needs the password grant, so
# locally a throwaway CI-config dex), DRILL_SOURCE_PORT (5441), DRILL_RESTORE_PORT (5442),
# DRILL_SNAPSHOT_WAIT seconds (600), DRILL_LOG_DIR (target/drill).
# Exit codes: 0 passed; 1 a check failed (a defect; serve exiting before ready on a restored
# database is one); 2 usage; 3 not ready (dex, Docker, the stacks, serve on the source, or no
# periodic snapshot within the wait): infrastructure, no verdict on the code.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
issuer="${E2E_ISSUER:-http://localhost:5556/dex}"
src_port="${DRILL_SOURCE_PORT:-5441}"
dst_port="${DRILL_RESTORE_PORT:-5442}"
snapshot_wait="${DRILL_SNAPSHOT_WAIT:-600}"
log_dir="${DRILL_LOG_DIR:-target/drill}"
export SQLX_OFFLINE=true
src_project=voidforge-drill-source
dst_project=voidforge-drill-restore
db=voidforge_drill
password=voidforge-local-test-password
say() { echo "backup-drill: $*" >&2; }

# Usage errors first, before the EXIT trap.
for p in "$src_port" "$dst_port" "$snapshot_wait"; do
    case "$p" in ''|*[!0-9]*) say "DRILL_SOURCE_PORT, DRILL_RESTORE_PORT and DRILL_SNAPSHOT_WAIT are whole numbers"; exit 2 ;; esac
done
[ "$src_port" != 5432 ] && [ "$dst_port" != 5432 ] || { say "port 5432 is the stack's; the drill never uses it"; exit 2; }
[ "$src_port" != "$dst_port" ] || { say "the source and the restore need their own ports"; exit 2; }
mkdir -p "$log_dir"
rm -f "$log_dir"/*.log "$log_dir"/*.json "$log_dir"/*.dump "$log_dir"/*.out

# Each stack's compose, always with -p (P5.2 R1-1), and psql inside its container.
dc() { local project=$1 port=$2; shift 2; DRILL_PG_PORT="$port" docker compose -f deploy/compose.drill.yaml -p "$project" "$@"; }
src() { dc "$src_project" "$src_port" "$@"; }
dst() { dc "$dst_project" "$dst_port" "$@"; }
psql_in() { local side=$1 database=$2; shift 2; "$side" exec -T postgres psql -U voidforge -X -q -v ON_ERROR_STOP=1 -At -d "$database" "$@" < /dev/null; }
# The drill's URLs: only its own two ports and voidforge_drill* databases (the shared guard).
url() {
    local port=$1 name=$2 u
    case "$port" in "$src_port"|"$dst_port") ;; *) say "refusing port $port: the drill uses $src_port and $dst_port only"; exit 2 ;; esac
    u="postgres://voidforge:$password@localhost:$port/$name"
    "$here/scratch-db.sh" "$u" "$db" "${db}_*" > /dev/null || exit 2
    echo "$u"
}

outcome=failed
why=""
pids=()
summary_line() { echo "$1" >> "$log_dir/summary-lines.log"; }
cleanup() {
    local code=$?
    for pid in "${pids[@]}"; do kill -KILL "$pid" 2> /dev/null || true; done
    for side in src dst; do "$side" down -v --remove-orphans > /dev/null 2>&1 || say "couldn't take down the $side stack: run the compose down -v by hand"; done
    { echo "## Backup and restore drill: $outcome"; echo; [ -n "$why" ] && { echo "$why"; echo; }
      cat "$log_dir/summary-lines.log" 2> /dev/null; } > "$log_dir/summary.log"
    cat "$log_dir/summary.log"
    [ -n "${GITHUB_STEP_SUMMARY-}" ] && cat "$log_dir/summary.log" >> "$GITHUB_STEP_SUMMARY"
    exit "$code"
}
finish() { outcome=$1; why=$2; say "$outcome: $why"; case "$outcome" in passed) exit 0 ;; infra) exit 3 ;; *) exit 1 ;; esac; }

# A stack already up is another run's, or a killed one's: never take it over.
for project in "$src_project" "$dst_project"; do
    if [ -n "$(docker ps -a -q --filter "label=com.docker.compose.project=$project")" ]; then
        say "the $project project is already up (another drill, or a killed one): take it down with"
        say "  DRILL_PG_PORT=0 docker compose -f deploy/compose.drill.yaml -p $project down -v"
        exit 3
    fi
done
trap cleanup EXIT

# 1. One build of serve, the bots and the replay.
cargo build -q --locked -p voidforge-server -p voidforge-bots -p voidforge-replay || finish failed "the build failed"
target_dir=$(cargo metadata -q --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
bin="$target_dir/debug"

# 2. dex.
grant=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$issuer/token" \
    -d 'grant_type=password&username=bot1%40voidforge.test&password=bot1-local-test-password&client_id=voidforge-cli&scope=openid' \
    || true)
[ "$grant" = 200 ] || finish infra "dex at $issuer doesn't grant bot1 a token (HTTP $grant): it needs deploy/dex/config.ci.yaml (locally, a throwaway dex on its own port: E2E_ISSUER)"

# 3. The two stacks.
src up --detach --wait --wait-timeout 120 > "$log_dir/compose-source.log" 2>&1 || finish infra "the source stack isn't healthy (see $log_dir/compose-source.log)"
dst up --detach --wait --wait-timeout 120 > "$log_dir/compose-restore.log" 2>&1 || finish infra "the restore stack isn't healthy (see $log_dir/compose-restore.log)"
src_url=$(url "$src_port" "$db") || exit 2
dst_url=$(url "$dst_port" "$db") || exit 2
say "source on :$src_port, restore on :$dst_port"

# serve on a database: its pid in $serve_pid once it prints ready, its log in $1. If serve exits
# before ready, it returns 1 with the exit code in $serve_exit, and the caller says what that means:
# on a restored database, the restore isn't usable. Alive but not ready within the wait is infra.
start_serve() {
    local name=$1 u=$2 addr=""
    serve_exit=""
    VOIDFORGE_DATABASE_URL="$u" "$bin/voidforge-server" serve --rules rules/v0-fast.toml \
        --bind 127.0.0.1:0 --issuer "$issuer" > "$log_dir/serve-$name.log" 2>&1 &
    serve_pid=$!
    pids+=("$serve_pid")
    for _ in $(seq 1 600); do
        addr=$(sed -n 's/^ready: listening on \([^,]*\),.*/\1/p' "$log_dir/serve-$name.log")
        [ -n "$addr" ] && break
        kill -0 "$serve_pid" 2> /dev/null || break
        sleep 0.2
    done
    if [ -z "$addr" ]; then
        kill -0 "$serve_pid" 2> /dev/null && finish infra "serve on the $name isn't ready within 120 s (see $log_dir/serve-$name.log)"
        wait "$serve_pid"; serve_exit=$?
        return 1
    fi
    serve_addr=$addr
    serve_ready_seq=$(sed -n 's/^ready: .*, seq \([0-9]*\)$/\1/p' "$log_dir/serve-$name.log")
}
stop_serve() {
    local name=$1 code
    kill -TERM "$serve_pid"
    for _ in $(seq 1 400); do kill -0 "$serve_pid" 2> /dev/null || break; sleep 0.1; done
    kill -0 "$serve_pid" 2> /dev/null && finish failed "serve on the $name didn't stop within 40 s of SIGTERM"
    wait "$serve_pid"; code=$?
    [ "$code" = 0 ] || finish failed "serve on the $name exited $code, not 0, after SIGTERM (see $log_dir/serve-$name.log)"
}
head_seq() { psql_in "$1" "$2" -c "SELECT coalesce(max(seq), 0) FROM log_records"; }

# 4. The source: a universe for eight bots, serve, and the bots playing.
VOIDFORGE_DATABASE_URL="$src_url" "$bin/voidforge-server" seed --seed 20261024 --players 8 \
    --rules rules/v0-fast.toml > "$log_dir/seed.log" 2>&1 || finish failed "seed failed (see $log_dir/seed.log)"
start_serve source "$src_url" || finish infra "serve on the source exited $serve_exit before ready (see $log_dir/serve-source.log)"
# A backup from before any periodic snapshot, for a negative: the check must refuse it.
src exec -T postgres pg_dump -U voidforge --format=custom --no-owner -d "$db" > "$log_dir/early.dump" 2>> "$log_dir/pg_dump.log" \
    || finish failed "pg_dump failed (see $log_dir/pg_dump.log)"
"$bin/voidforge-bots" sim --api "http://$serve_addr" --issuer "$issuer" --run "drill$$" --seed 24 \
    --minutes 30 > "$log_dir/bots-source.out" 2> "$log_dir/bots-source.log" &
bots_pid=$!
pids+=("$bots_pid")

# 5. The first periodic snapshot, bounded, then a tail after it.
started=$(date +%s)
while :; do
    periodic=$(psql_in src "$db" -c "SELECT count(*) FROM snapshots WHERE seq > 0" 2> /dev/null || echo 0)
    [ "$periodic" != 0 ] && break
    kill -0 "$bots_pid" 2> /dev/null || finish failed "the bots stopped before the first periodic snapshot (see $log_dir/bots-source.log)"
    [ $(( $(date +%s) - started )) -lt "$snapshot_wait" ] \
        || finish infra "no periodic snapshot within $snapshot_wait s (serve snapshots every 1,000 records; head $(head_seq src "$db"))"
    sleep 5
done
say "the source's first periodic snapshot after $(( $(date +%s) - started )) s; a tail follows"
sleep 30

# 6. The backup, live: serve and the bots still running.
kill -0 "$serve_pid" 2> /dev/null && kill -0 "$bots_pid" 2> /dev/null || finish failed "serve or the bots stopped before the backup"
before=$(head_seq src "$db")
src exec -T postgres pg_dump -U voidforge --format=custom --no-owner -d "$db" > "$log_dir/drill.dump" 2> "$log_dir/pg_dump.log" \
    || finish failed "pg_dump failed (see $log_dir/pg_dump.log)"
after=$(head_seq src "$db")
kill -0 "$serve_pid" 2> /dev/null && kill -0 "$bots_pid" 2> /dev/null || finish failed "serve or the bots stopped during the backup"
size=$(wc -c < "$log_dir/drill.dump")
say "backed up $size bytes while the source's head went from $before to $after"
# The source goes on past the backup, so the backup is a prefix of a log that kept growing, and
# the source's hash at B isn't simply its head's.
sleep 15

# 7. Stop the bots (they were cut short on purpose; their checks held while they ran), then serve.
kill -TERM "$bots_pid" 2> /dev/null; wait "$bots_pid" 2> /dev/null
grep -q -E 'sim (FAILED|HTTP|LOGIN|DEADLINE)' "$log_dir/bots-source.log" \
    && finish failed "a bot check failed on the source (see $log_dir/bots-source.log)"
stop_serve source
source_head=$(head_seq src "$db")

# 8. The restore, into the fresh stack's empty database.
dst exec -T postgres pg_restore -U voidforge --exit-on-error --single-transaction --no-owner -d "$db" \
    < "$log_dir/drill.dump" > "$log_dir/pg_restore.log" 2>&1 \
    || finish failed "the restore failed: the backup is truncated or corrupt (see $log_dir/pg_restore.log)"

# 9. B, the hashes, the tables.
"$bin/voidforge-replay" inspect postgres --database-url "$dst_url" --json > "$log_dir/inspect-restored.json" 2>> "$log_dir/replay.log" \
    || finish failed "the replay can't inspect the restored log (see $log_dir/replay.log)"
B=$(head_seq dst "$db")
# Each comparison is a function the negatives call too, so a broken one fails the drill.
has_periodic() {
    python3 -c '
import json, sys
s = json.load(open(sys.argv[1]))["sources"][0]["snapshots"]
sys.exit(0 if any(x["seq"] > 0 for x in s) else 1)' "$1"
}
same_hash() { [ -n "$1" ] && [ "$1" = "$2" ]; }
same_tables() { [ -n "$1" ] && [ "$1" = "$2" ]; }
has_periodic "$log_dir/inspect-restored.json" \
    || finish failed "the backup holds no periodic snapshot (only the seed's): it was taken too early"
[ "$B" -gt 0 ] && [ "$B" -lt "$source_head" ] \
    || finish failed "the restored head $B isn't inside the source's log, which went on to $source_head after the backup"
[ "$before" -le "$B" ] && [ "$B" -le "$after" ] \
    || finish failed "the restored head $B is outside the source's heads around the backup ($before to $after): the restore lost the log's tail"
"$bin/voidforge-replay" postgres --database-url "$dst_url" --rules rules/v0-fast.toml --json \
    > "$log_dir/replay-restored.json" 2>> "$log_dir/replay.log" \
    || finish failed "the restored log doesn't verify (see $log_dir/replay-restored.json)"
"$bin/voidforge-replay" postgres --database-url "$src_url" --rules rules/v0-fast.toml --to "$B" --json \
    > "$log_dir/replay-source.json" 2>> "$log_dir/replay.log" \
    || finish failed "the source's log doesn't verify to seq $B (see $log_dir/replay-source.json)"
read -r restored_seq restored_hash < <(python3 -c 'import json,sys; s=json.load(open(sys.argv[1]))["sources"][0]; print(s["last_seq"], s["end_hash"])' "$log_dir/replay-restored.json")
read -r source_seq source_hash < <(python3 -c 'import json,sys; s=json.load(open(sys.argv[1]))["sources"][0]; print(s["last_seq"], s["end_hash"])' "$log_dir/replay-source.json")
[ "$restored_seq" = "$B" ] && [ "$source_seq" = "$B" ] \
    || finish failed "the replays stopped at $restored_seq (restored) and $source_seq (source), not both at $B"
same_hash "$restored_hash" "$source_hash" \
    || finish failed "the state hash at seq $B differs: restored $restored_hash, source $source_hash"
# Every table, up to B: the hash covers the log, not the accounts or the epoch. The snapshots are
# compared below B on both sides, plus at B when the restored copy has one. serve awaits each
# snapshot before the next turn's append (voidforge-host lib.rs), so every source snapshot below B
# committed before record B and is in the backup. One at B may have come just after the backup, and
# the source's later ones (its shutdown snapshot) did. The bound never comes from the restored list
# alone, so a restore that lost a snapshot row compares unequal.
snapshot_at_b=$(psql_in dst "$db" -c "SELECT count(*) FROM snapshots WHERE seq = $B") || finish infra "can't read the restored snapshots"
snapshot_seqs=$(psql_in dst "$db" -c "SELECT coalesce(string_agg(seq::text, ',' ORDER BY seq), '-') FROM snapshots")
fingerprint() {
    psql_in "$1" "${2:-$db}" -c "SELECT
        (SELECT count(*) || ':' || md5(coalesce(string_agg(t::text, ',' ORDER BY seq), '')) FROM log_records t WHERE seq <= $B),
        (SELECT count(*) || ':' || md5(coalesce(string_agg(t::text, ',' ORDER BY seq), '')) FROM snapshots t WHERE seq < $B OR (seq = $B AND $snapshot_at_b > 0)),
        (SELECT count(*) || ':' || md5(coalesce(string_agg(t::text, ',' ORDER BY player_id), '')) FROM accounts t),
        (SELECT count(*) || ':' || md5(coalesce(string_agg(t::text, ','), '')) FROM universe t)"
}
src_fp=$(fingerprint src) || finish infra "can't read the source's tables"
dst_fp=$(fingerprint dst) || finish infra "can't read the restored tables"
same_tables "$src_fp" "$dst_fp" || finish failed "the tables differ up to seq $B (records|snapshots|accounts|epoch): source $src_fp, restored $dst_fp"

# 10. The restored server: it recovers to B, the bots play a minute, and their commands are logged past B.
start_serve restored "$dst_url" \
    || finish failed "serve on the restored database exited $serve_exit before ready: the restore isn't usable (see $log_dir/serve-restored.log)"
[ "$serve_ready_seq" = "$B" ] || finish failed "serve on the restored database recovered to seq ${serve_ready_seq:-?}, not $B"
"$bin/voidforge-bots" sim --api "http://$serve_addr" --issuer "$issuer" --run "drillr$$" --seed 25 \
    --minutes 1 > "$log_dir/bots-restored.out" 2> "$log_dir/bots-restored.log" \
    || finish failed "the bots failed on the restored server (see $log_dir/bots-restored.log)"
stop_serve restored
"$bin/voidforge-replay" postgres --database-url "$dst_url" --rules rules/v0-fast.toml --json \
    > "$log_dir/replay-restored-after.json" 2>> "$log_dir/replay.log" \
    || finish failed "the restored log doesn't verify after the bots' minute (see $log_dir/replay-restored-after.json)"
after_seq=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sources"][0]["last_seq"])' "$log_dir/replay-restored-after.json")
[ "$after_seq" -gt "$B" ] || finish failed "the restored server appended nothing (head $after_seq, B $B)"
# The host also logs time-only advances, so a head past B alone doesn't show the bots' commands.
after_commands=$(psql_in dst "$db" -c "SELECT count(*) FROM log_records WHERE seq > $B AND kind = 'command'") \
    || finish infra "can't read the restored log"
[ "$after_commands" -gt 0 ] || finish failed "the restored server logged no command past B $B (head $after_seq): the bots' minute didn't reach it"

# 11. The negatives, from the same backup. Each must fail; if one passes, its check is broken.
psql_in dst "$db" -c "CREATE DATABASE ${db}_truncated" > /dev/null
psql_in dst "$db" -c "CREATE DATABASE ${db}_tampered" > /dev/null
head -c "$(( size / 2 ))" "$log_dir/drill.dump" > "$log_dir/truncated.dump"
if dst exec -T postgres pg_restore -U voidforge --exit-on-error --single-transaction --no-owner -d "${db}_truncated" \
    < "$log_dir/truncated.dump" > "$log_dir/negative-truncated.log" 2>&1; then
    finish failed "a backup cut in half restored without an error: the restore's check is broken"
fi
dst exec -T postgres pg_restore -U voidforge --exit-on-error --single-transaction --no-owner -d "${db}_tampered" \
    < "$log_dir/drill.dump" > "$log_dir/negative-tampered-restore.log" 2>&1 || finish failed "the clean backup didn't restore for the tamper case"
tampered=$(psql_in dst "${db}_tampered" -c "UPDATE log_records SET events = '[]'::jsonb
    WHERE seq = (SELECT seq FROM log_records WHERE events <> '[]'::jsonb AND seq <= $B ORDER BY seq OFFSET (SELECT count(*) / 2 FROM log_records WHERE events <> '[]'::jsonb AND seq <= $B) LIMIT 1)
    RETURNING seq")
[ -n "$tampered" ] || finish failed "no record with events to tamper in the backup"
tampered_url=$(url "$dst_port" "${db}_tampered") || exit 2
"$bin/voidforge-replay" postgres --database-url "$tampered_url" --rules rules/v0-fast.toml --json \
    > "$log_dir/negative-tampered.json" 2>> "$log_dir/replay.log"
tampered_code=$?
[ "$tampered_code" = 1 ] || finish failed "a restored log with seq $tampered tampered replayed with exit $tampered_code, not 1 (diverged): the hash check is broken"
# The tables' comparison tells the tampered copy from the source.
tampered_fp=$(fingerprint dst "${db}_tampered") || finish infra "can't read the tampered copy's tables"
! same_tables "$src_fp" "$tampered_fp" || finish failed "the tampered copy's tables compare equal to the source's: the tables' check is broken"
# A restored copy that lost a snapshot row compares unequal; without its epoch, serve exits before ready.
psql_in dst "$db" -c "CREATE DATABASE ${db}_partial" > /dev/null
dst exec -T postgres pg_restore -U voidforge --exit-on-error --single-transaction --no-owner -d "${db}_partial" \
    < "$log_dir/drill.dump" > "$log_dir/negative-partial-restore.log" 2>&1 || finish failed "the clean backup didn't restore for the partial case"
lost=$(psql_in dst "${db}_partial" -c "DELETE FROM snapshots WHERE seq = (SELECT min(seq) FROM snapshots) RETURNING seq")
[ -n "$lost" ] || finish failed "no snapshot to delete in the backup"
partial_fp=$(fingerprint dst "${db}_partial") || finish infra "can't read the partial copy's tables"
! same_tables "$src_fp" "$partial_fp" || finish failed "a restored copy without its snapshot at seq $lost compares equal to the source's: the tables' check is broken"
psql_in dst "${db}_partial" -c "DELETE FROM universe" > /dev/null
partial_url=$(url "$dst_port" "${db}_partial") || exit 2
if start_serve partial "$partial_url"; then
    stop_serve partial
    finish failed "serve started on a restored copy without its epoch: the restored server's check is broken"
fi
[ "$serve_exit" = 3 ] || finish failed "serve on a restored copy without its epoch exited $serve_exit, not 3 (see $log_dir/serve-partial.log)"
# The hash comparison tells two worlds apart: the source halfway to B isn't the restored one.
# Halfway, not one record back: a record can leave the world as it was (a refused command, as on
# CI's first run, where seq B and B-1 hashed alike), but hundreds of accepted ones can't.
half=$(( B / 2 ))
"$bin/voidforge-replay" postgres --database-url "$src_url" --rules rules/v0-fast.toml --to "$half" --json \
    > "$log_dir/negative-earlier.json" 2>> "$log_dir/replay.log" || finish failed "the source's log doesn't verify to seq $half"
earlier_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sources"][0]["end_hash"])' "$log_dir/negative-earlier.json")
! same_hash "$restored_hash" "$earlier_hash" || finish failed "the hashes at seq $B and seq $half compare equal: the hash check is broken"
# The drill's own guard refuses the stack's port.
( url 5432 "$db" ) > /dev/null 2>&1 && finish failed "the drill's URL check accepted port 5432: it's broken"
# A backup from before the first periodic snapshot is refused.
psql_in dst "$db" -c "CREATE DATABASE ${db}_early" > /dev/null
dst exec -T postgres pg_restore -U voidforge --exit-on-error --single-transaction --no-owner -d "${db}_early" \
    < "$log_dir/early.dump" > "$log_dir/negative-early-restore.log" 2>&1 || finish failed "the early backup didn't restore"
early_url=$(url "$dst_port" "${db}_early") || exit 2
"$bin/voidforge-replay" inspect postgres --database-url "$early_url" --json > "$log_dir/negative-early.json" 2>> "$log_dir/replay.log" \
    || finish failed "the replay can't inspect the early backup"
! has_periodic "$log_dir/negative-early.json" || finish failed "a backup from before any periodic snapshot passed the snapshot check: it's broken"

summary_line "- Source: 8 bots on the fast rules; backed up live ($size bytes) while its head went from $before to $after; stopped at $source_head."
summary_line "- Restored head B = $B (snapshots: $(python3 -c 'import json,sys; print(",".join(str(x["seq"]) for x in json.load(open(sys.argv[1]))["sources"][0]["snapshots"]))' "$log_dir/inspect-restored.json"))."
summary_line "- State hash at seq $B, by voidforge-replay on both sides: restored $restored_hash, source $source_hash (equal)."
summary_line "- Tables up to seq $B, equal (records|snapshots at $snapshot_seqs|accounts|epoch): $dst_fp."
summary_line "- The restored server recovered to seq $B; the bots played a minute on it; its log verifies to seq $after_seq, with $after_commands commands past B."
summary_line "- Negatives: a truncated backup failed pg_restore ($(grep -m1 -i error "$log_dir/negative-truncated.log" | cut -c1-100)); a restored log with seq $tampered tampered diverged (exit 1), and its tables compare unequal; a restored copy without its snapshot at seq $lost compares unequal, and without its epoch serve exits 3 before ready; the source's hash at seq $half differs from B's; the drill's URL check refuses port 5432; a backup taken before any periodic snapshot is refused."
finish passed "the restored state hash at seq $B matches the source's"
