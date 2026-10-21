# voidforge-telemetry

Tracing and OpenTelemetry setup: `init`, used by every binary, installs the `tracing` subscriber, logs locally and exports traces and metrics over OTLP/HTTP when an endpoint is configured. `traceparent` and `set_remote_parent` carry a trace across HTTP; `meter` is where crates get their instruments ([D-064](../../docs/decision-log.md#d-064-telemetry-a-commands-trace-the-hosts-metrics-and-trace-context-across-http)).

Library, delivered in P0.6 ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): tracing, tracing-subscriber, opentelemetry, opentelemetry_sdk, opentelemetry-otlp (OTLP/HTTP with the blocking reqwest client, no TLS), tracing-opentelemetry and thiserror, at the versions in the [version table](../../docs/versions.md) ([D-017](../../docs/decision-log.md#d-017-telemetry-dependencies-licenses-and-duplicates)).
- Never: Any workspace crate.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry is set up once, here. Binaries call `init` instead of configuring exporters themselves; a second `init` in one process is an error.
- `OTEL_EXPORTER_OTLP_ENDPOINT` decides export. Set (locally `http://localhost:4318`), spans go to `<endpoint>/v1/traces` and metrics to `<endpoint>/v1/metrics` every 10 s. Unset, the binary still runs and logs to stderr, instruments are no-ops, and nothing waits on a collector.
- Recording never waits on export: spans queue in a bounded batch, and both exporters run on their own threads.
- An incoming request joins a caller's trace through `traceparent` only: never `tracestate` or baggage from outside (D-064 T3).
- `init_quieting` drops only the events its `Quiet` list names (the web: tower-sessions' "store is full" ERROR, which it reports itself); every other event at INFO and above passes.
- `Telemetry::shutdown` flushes the spans, always stops the exporters (even after a failed flush), and reports what it could not deliver as an error. Spans and metrics shut down side by side, so a collector that doesn't answer costs about one 5-second export timeout. Dropping the guard does the same and prints a failure to stderr. `serve` and the web report that error without changing their exit code (D-064 T2).
- The core stays free of instrumentation, and the shell records the step statistics the core returns ([Operations and scaling](../../docs/Voidforge%20Architectural%20Design.md#operations-and-scaling)).

## Tests

```sh
cargo build -p voidforge-telemetry
cargo clippy -p voidforge-telemetry --all-targets -- -D warnings
cargo nextest run -p voidforge-telemetry
```

Unit tests cover:
- delivery of a span to an OTLP endpoint;
- the delivery error when nothing listens;
- a rejected flush still stopping the exporter, so nothing recorded afterwards is exported;
- the offline path;
- the second-init error;
- metrics delivered to `/v1/metrics`, a traceparent round trip reading only `traceparent`, the filter dropping only its event, and a hung collector never slowing recording.

The tests use a scoped subscriber, apart from the second-init test. The `testing` feature (dev-only, for other crates' tests) captures spans and metrics in memory, once per process. `just trace-check` proves every binary's span reaches Grafana's trace backend.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Operations and scaling](../../docs/Voidforge%20Architectural%20Design.md#operations-and-scaling), [Local development](../../docs/Voidforge%20Technical%20Plan.md#local-development), [Phase 5](../../docs/Voidforge%20Technical%20Plan.md#phase-5-operations-and-packaging)
