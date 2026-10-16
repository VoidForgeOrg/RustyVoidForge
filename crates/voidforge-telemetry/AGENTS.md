# voidforge-telemetry

Tracing and OpenTelemetry setup: `init`, used by every binary, installs the `tracing` subscriber, logs locally and exports traces over OTLP/HTTP when an endpoint is configured. Metrics arrive with P5.1.

Library, delivered in P0.6 ([D-016](../../docs/decision-log.md#d-016-telemetry-bootstrap)). The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): tracing, tracing-subscriber, opentelemetry, opentelemetry_sdk, opentelemetry-otlp (OTLP/HTTP with the blocking reqwest client, no TLS), tracing-opentelemetry and thiserror, at the versions in the [version table](../../docs/versions.md) ([D-017](../../docs/decision-log.md#d-017-telemetry-dependencies-licenses-and-duplicates)).
- Never: Any workspace crate.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry is set up once, here. Binaries call `init` instead of configuring exporters themselves; a second `init` in one process is an error.
- `OTEL_EXPORTER_OTLP_ENDPOINT` decides export. Set (locally `http://localhost:4318`), spans go to `<endpoint>/v1/traces`. Unset, the binary still runs and logs to stderr, and nothing waits on a collector.
- `Telemetry::shutdown` flushes, always stops the exporter (even after a failed flush), and reports spans it could not deliver as an error, within the 5-second export timeout. Dropping the guard flushes too and prints a failure to stderr.
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
- the second-init error.

The tests use a scoped subscriber, apart from the second-init test. `just trace-check` proves every binary's span reaches Grafana's trace backend.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Operations and scaling](../../docs/Voidforge%20Architectural%20Design.md#operations-and-scaling), [Local development](../../docs/Voidforge%20Technical%20Plan.md#local-development), [Phase 5](../../docs/Voidforge%20Technical%20Plan.md#phase-5-operations-and-packaging)
