# Voidforge operations runbook

Step-by-step procedures for an operator, beside the compose files they name. `deploy/README.md` is the stack's manual: its addresses, telemetry, images and troubleshooting. Values in examples are the local stack's TEST-ONLY ones.

## Backup and restore

A universe lives entirely in its Postgres database: the log (`log_records`), the snapshots, the players' accounts and the epoch (`universe`). A backup is a `pg_dump` of that one database. It can be taken **while the server runs**: pg_dump reads every table inside one transaction, and the server writes each turn's records in one transaction, so a backup always holds the whole log up to some record, never half a turn ([D-067](../docs/decision-log.md#d-067-the-backup-and-restore-drill)).

The restored world is checked by its **state hash**: the replay tool hashes the restored log at its last record, B, and the source's log at the same record. Equal hashes mean the same world, record for record. Use the `voidforge-replay` built from the same commit as the server: it refuses a log written by another build (exit 3) unless you name that build with `--build`.

`just backup-drill` rehearses all of this on two scratch stacks of its own. Run it before you need it.

In the commands below, `$SOURCE_URL` is the running universe's database URL and `$TARGET_URL` the fresh one's. `$SRC` and `$DST` run commands inside their Postgres containers, for example `docker compose -f deploy/compose.yaml -p voidforge exec -T postgres` for the stack's. Always name the compose project with `-p`.

### 1. Back up (the server can keep running)

```sh
$SRC pg_dump -U voidforge --format=custom --no-owner -d voidforge > voidforge-$(date -u +%Y%m%dT%H%M%SZ).dump
```

- Custom format (`-Fc`) has a table of contents, so a damaged file fails the restore instead of loading part of the universe.
- Check that the file isn't empty, and keep it away from the machine it came from.
- A failed `pg_dump` exits non-zero with its reason. Don't keep that file.

### 2. Bring up a fresh Postgres

The same Postgres version as the source (the pin in `deploy/compose.yaml`), with an empty database. On another machine that can be the stack itself (`just up`). Beside a running stack, use another project and port: `deploy/compose.drill.yaml` is one, with `DRILL_PG_PORT=<port> docker compose -f deploy/compose.drill.yaml -p <project> up --detach --wait`, which creates an empty `voidforge_drill`.

### 3. Restore

```sh
$DST pg_restore -U voidforge --exit-on-error --single-transaction --no-owner -d <empty database> < voidforge-….dump
```

- `--single-transaction --exit-on-error` restores all of it or nothing.
- A truncated or corrupt file fails here with `pg_restore: error: …` and leaves the database empty. Take another backup; never start a server on a partial restore.

### 4. Check the state hash

```sh
voidforge-replay postgres --database-url "$TARGET_URL" --rules rules/<the universe's rules>.toml --json
```

- It must say `"result": "verified"`. It replays the restored log from its earliest snapshot and checks every later snapshot on the way. Note its `last_seq` (B) and its `end_hash`.
- If the source is still at hand, hash it at the same record:

  ```sh
  voidforge-replay postgres --database-url "$SOURCE_URL" --rules rules/<rules>.toml --to <B> --json
  ```

  Both `end_hash` values must be equal.
- What failure looks like:
  - exit 1, `diverged`, with the first record that doesn't replay: the log was damaged;
  - exit 3: the replay tool is another build (use the server's own, or `--build`);
  - exit 4: it can't read the database.

### 5. Start the server on it

Start the server binary on the restored database, built from the same commit as the replay tool:

```sh
VOIDFORGE_DATABASE_URL="$TARGET_URL" voidforge-server serve --rules rules/<the universe's rules>.toml --bind <address> --issuer <the dex issuer>
```

It must print `ready: listening on …, seq B`: the restored head, recovered from the latest snapshot at or below it. (`just images-up` won't do here: it takes only the scratch databases `voidforge_dev` and `voidforge_e2e_*`.)

- It refuses a database without its epoch (exit 3). That means the restore missed the `universe` table, so it was partial. Its message suggests re-seeding: **don't**, since a seed would replace the universe you're restoring. Restore again from the backup, into an empty database.
- Players log in as before: their accounts came back with the universe.

### Rehearsal: `just backup-drill`

It runs steps 1 to 5 against two scratch stacks of its own (projects `voidforge-drill-source` and `voidforge-drill-restore`, ports 5441 and 5442), never the stack's:
- The source is a universe the eight test bots play on. Its backup is taken while they play, after serve's first periodic snapshot.
- The restore is checked by the hash at B and by every table up to B.
- The restored server must recover to B, then the bots play a minute more on it.
- A truncated backup, a tampered record and a restored copy that lost a snapshot must all fail, and serve must refuse a copy without its epoch.

It needs a dex with the password grant (`E2E_ISSUER`; locally a throwaway CI-config dex). It takes about 8 minutes, and CI runs it on every pull request.
