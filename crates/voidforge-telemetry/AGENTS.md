# voidforge-telemetry

Tracing and OpenTelemetry setup: one init function, used by every binary, that exports traces and metrics over OTLP.

Library. It is a stub today; P0.6 is the first package that fills it. The layering in [Project layering](../../docs/Voidforge%20Technical%20Plan.md#project-layering) is the authority, and the [root AGENTS.md](../../AGENTS.md) covers repository-wide rules.

## Allowed dependencies

- Workspace: None.
- External, per the [Tech stack](../../docs/Voidforge%20Technical%20Plan.md#tech-stack): tracing, opentelemetry, opentelemetry-otlp and tracing-opentelemetry.
- Never: Any workspace crate.
- Every new dependency needs a [decision log](../../docs/decision-log.md) entry, and its version goes in the [version table](../../docs/versions.md).

## Invariants

- Telemetry is set up once, here. Binaries call the init function instead of configuring exporters themselves.
- The core stays free of instrumentation, and the shell records the step statistics the core returns ([Operations and scaling](../../docs/Voidforge%20Architectural%20Design.md#operations-and-scaling)).

## Tests

```sh
cargo build -p voidforge-telemetry
cargo clippy -p voidforge-telemetry --all-targets -- -D warnings
cargo nextest run -p voidforge-telemetry --no-tests=warn   # drop --no-tests=warn once the workspace has a real test (D-004)
```

P0.6: a stub binary's span shows up in local Grafana.

## Specs

- [MVP Game Design](../../docs/Voidforge%20MVP%20Game%20Design.md), [Architectural Design](../../docs/Voidforge%20Architectural%20Design.md), [Technical Plan](../../docs/Voidforge%20Technical%20Plan.md)
- Most relevant: [Operations and scaling](../../docs/Voidforge%20Architectural%20Design.md#operations-and-scaling), [Local development](../../docs/Voidforge%20Technical%20Plan.md#local-development), [Phase 5](../../docs/Voidforge%20Technical%20Plan.md#phase-5-operations-and-packaging)
