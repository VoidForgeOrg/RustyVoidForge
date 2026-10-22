# Local stack

Postgres, dex and Grafana otel-lgtm for local runs and CI ([Technical Plan, Local development](../docs/Voidforge%20Technical%20Plan.md#local-development); decision log [D-014](../docs/decision-log.md#d-014-local-stack) and [D-015](../docs/decision-log.md#d-015-one-dex-issuer-for-the-browser-the-host-and-containers)).

```sh
just up                               # build, start, wait until healthy, verify by effect
DEX_CONFIG=config.ci.yaml just up     # the same with the CI dex config (password grant on)
just status                           # verify the running stack again
just logs dex                         # follow one service's logs
just down                             # stop; Postgres data stays in the voidforge_postgres-data volume
docker compose -f deploy/compose.yaml down --volumes   # stop and delete this stack's Postgres data
```

`just up` is safe to repeat: it leaves healthy containers alone and recreates dex only when its config changes. It exits non-zero and names the failing check if anything is unhealthy or wrong.

## Addresses

Every port is published on 127.0.0.1 only.

| Service | Address | Health |
| --- | --- | --- |
| Postgres | `postgres://voidforge:voidforge-local-test-password@localhost:5432/voidforge_dev` (the development universe; `voidforge` is the owner's, and nothing defaults to it, D-055) | `pg_isready` |
| dex | issuer `http://localhost:5556/dex`; discovery at `/dex/.well-known/openid-configuration`, keys at `/dex/keys` | discovery answers |
| Grafana | `http://localhost:3000` | every otel-lgtm component ready |
| OTLP | gRPC `localhost:4317`, HTTP `localhost:4318` | |

## Test-only credentials

These values exist only for local runs and CI. They are not secrets, and they must never be reused anywhere real.

| What | Value |
| --- | --- |
| Postgres user / password / database | `voidforge` / `voidforge-local-test-password` / `voidforge` |
| dex user | `alice@voidforge.test` / `alice-local-test-password` |
| dex user | `bob@voidforge.test` / `bob-local-test-password` |
| Web app client (public, authorization code with PKCE in the browser; dex allows its origin) | `voidforge-spa`, redirect `http://localhost:8081/callback` |
| CLI client (public, device code) | `voidforge-cli`, device callback `/dex/device/callback` (the issuer's path plus `/device/callback`: dex builds it that way, so a bare `/device/callback` refuses every approval, D-053) |

The password grant for the static users is enabled only in `dex/config.ci.yaml`. Client credentials are never enabled.

## Telemetry

Binaries export traces and metrics when `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318` is set, and only log locally without it ([D-016](../docs/decision-log.md#d-016-telemetry-bootstrap), [D-064](../docs/decision-log.md#d-064-telemetry-a-commands-trace-the-hosts-metrics-and-trace-context-across-http)). Grafana at `http://localhost:3000` shows them in **Explore**, with no dashboard to set up. Undelivered telemetry is an ERROR on stderr and never changes an exit code (only `telemetry-check` fails on it).

- **A command's trace** (Explore > Tempo, query type TraceQL):
  - `{ name = "web.act" } && { name = "host.append" }` finds a browser form's trace;
  - `{ name = "host.command" && span.outcome != "accepted" }` finds rejected commands.
  - A trace runs `web.act` (voidforge-web) → `api.request` (voidforge-server; `http.route`, the status) → `host.inbox` (the wait) and `host.command` (`command.type`, `player_id` as the API's UUID, `seq`, `outcome`) → `host.step` and `host.append` (`batch.size`). The bots' commands start under their `e2e` span.
- **The host's metrics** (Explore > Prometheus):

  | Metric | Query |
  |---|---|
  | Inbox depth (commands waiting) | `voidforge_host_inbox_depth` |
  | Event lag (game-time ms; 0 unless a due event waits) | `voidforge_host_event_lag_milliseconds` |
  | Step latency, p99 by kind | `histogram_quantile(0.99, sum by (le, kind) (rate(voidforge_host_step_duration_seconds_bucket[5m])))` |
  | Append latency, p99 | `histogram_quantile(0.99, sum by (le) (rate(voidforge_host_append_duration_seconds_bucket[5m])))` |
  | Snapshot age (game-time s) | `voidforge_host_snapshot_age_seconds` |
  | Leaderboard builds failing | `rate(voidforge_leaderboard_failures_total[5m])` (exported once a build fails; the name follows the collector's counter rule, not yet seen live) |

  Event lag and snapshot age are game time, which is wall time while the game clock never pauses.
- `just trace-check` runs each binary's hidden `telemetry-check` and finds its span in Tempo.

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 cargo run -p voidforge-server -- telemetry-check   # prints trace_id=<hex>
just trace-check   # every binary: export, then find its span through Grafana's Tempo datasource
```

## Containers and the issuer

The issuer is `http://localhost:5556/dex` everywhere. A container that needs it runs with host networking, so `localhost:5556` reaches dex as it does from the host. It then reaches Postgres and the other services through their `localhost` ports, not compose service names. `just up` proves this with a host-network probe container.

## The images (P5.2)

`dockerfiles/Dockerfile` builds `voidforge-server:local` and `voidforge-web:local`: distroless, no shell, running as 65532:65532. Nothing is pushed. They run under `deploy/compose.images.yaml`, a project of its own (`voidforge-images`) beside this stack, never inside it, so compose never recreates the stack's containers (D-065).

```sh
just images                                        # build both, then check user, size and contents (`just images 1` for one build job)
E2E_ISSUER=http://localhost:5563/dex just images-compose   # both up healthy on a scratch database, stopped cleanly, taken down
E2E_ISSUER=http://localhost:5563/dex just smoke-images     # the smoke test against the images
VOIDFORGE_DATABASE_URL=postgres://…/<scratch> just images-up   # leave both running; `just images-down` stops them
```

- `VOIDFORGE_DATABASE_URL` is required and has no default: name a seeded scratch database, never `voidforge`.
- `docker compose stop` sends SIGTERM to serve itself (PID 1); it drains and stops with its final snapshot, exit 0. SIGKILL comes only after 40 s.
- Health: serve's is `voidforge-server health` on `/openapi.json`, the web's `voidforge-web --health-check` on `/`. Serve's start period is 300 s, so a long recovery reads "starting", never "unhealthy".
- On colima, "host" is the VM's network; colima forwards each listening port to the Mac with an ssh listener that outlives a stopped container by up to ~3 s. The web walk waits for its own previous web's port to come free, and after its web's receipt waits for the forward (20-30 ms). A known colima limit (D-065): in that gap the receipt proves the bind in the VM, not on the Mac; Linux has no forward and no gap.

## Troubleshooting

- **"A host-network container could not reach dex"**: Docker Desktop needs Settings > Resources > Network > "Enable host networking". This stack is verified with colima; Docker on a Linux host supports host networking by design; the CI images job (P5.2) runs both images that way.
- **"docker: unknown command: docker compose"**: install the Compose plugin (Compose 2.17 or later for `up --wait`).
- **A port is already in use**: something else listens on 5432, 5556, 3000, 4317 or 4318. Stop it, or stop the other stack; this stack never stops containers that are not its own.
- **"Unregistered redirect_uri" after entering a device code**: dex's device callback is the issuer's path plus `/device/callback`, so `voidforge-cli` must register `/dex/device/callback`. A running dex keeps the config it was built with: run `just up` to rebuild it after the config changes (D-053).
- **Why dex is built, not mounted**: the dex config is copied into a small local image, because some Docker VMs cannot read the checkout (for example colima with the repository under `~/Documents`) (D-014).
