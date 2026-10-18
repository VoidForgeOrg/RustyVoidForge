# The database stage's lock (decision log D-044): one database.sh run at a time per Postgres
# server. Sourced by database.sh and by its regression, test-database-lock.sh.
#
# The caller defines `sql` (one statement against the server's maintenance database), `fail`
# (report and exit 2), `psql` (the psql command, as an array) and `lock_db` (the lock's name).
#
# The lock is a database: CREATE DATABASE is atomic, so exactly one run creates it. Its comment
# names the holder's host and pid. Nothing ever drops another run's lock: a lock whose holder is
# gone (a run killed before its exit trap) is refused with the command that clears it, because two
# waiters that both saw a dead holder could otherwise both take over (phase 2 gate, review-r2 M1).

lock_holder="$(hostname) $$"
lock_held=

lock_owner() {
    sql "SELECT coalesce(shobj_description(oid, 'pg_database'), '')
         FROM pg_database WHERE datname = '$lock_db'"
}

# The command that clears a lock by hand, for the messages.
lock_clear_command() {
    printf "%s -d postgres -c 'DROP DATABASE %s'" "${psql[*]}" "$lock_db"
}

# Takes the lock, waiting up to VOIDFORGE_DB_LOCK_WAIT seconds (default 1800) for a live holder.
lock_acquire() {
    local wait=${VOIDFORGE_DB_LOCK_WAIT:-1800} waited=0 owner said= missing=0 error
    until error=$(sql "CREATE DATABASE $lock_db" 2>&1); do
        owner=$(lock_owner)
        if [ -z "$(sql "SELECT 1 FROM pg_database WHERE datname = '$lock_db'")" ]; then
            # Released between the two statements: try again, unless creating it keeps failing.
            missing=$((missing + 1))
            [ "$missing" -lt 3 ] || fail "cannot create the lock database $lock_db: $error"
            continue
        fi
        # A holder on this host that is no longer running never released the lock.
        if [ -n "$owner" ] && [ "${owner% *}" = "$(hostname)" ] \
            && ! ps -p "${owner##* }" > /dev/null 2>&1; then
            fail "the lock $lock_db is held by a run that is no longer running ($owner). \
If no database-stage run is active, clear it, then run again: $(lock_clear_command)"
        fi
        [ -n "$said" ] || { echo "waiting for another database-stage run (${owner:-starting}) to finish"; said=1; }
        [ "$waited" -lt "$wait" ] || fail "another run (${owner:-starting}) still holds $lock_db after \
${wait}s. If no database-stage run is active, clear it: $(lock_clear_command)"
        sleep 5
        waited=$((waited + 5))
    done
    lock_held=1
    sql "COMMENT ON DATABASE $lock_db IS '$lock_holder'"
}

# Releases the lock if this run holds it, and only while its comment still names this run.
lock_release() {
    [ -n "$lock_held" ] || return 0
    local owner
    owner=$(lock_owner)
    if [ -n "$owner" ] && [ "$owner" != "$lock_holder" ]; then
        echo "warning: $lock_db now names another holder ($owner); left alone" >&2
        return 0
    fi
    sql "DROP DATABASE IF EXISTS $lock_db"
    lock_held=
}
