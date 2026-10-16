//! API, token checks, accounts and the leaderboard job.
//!
//! A stub until its feature packages land. It already initializes telemetry the shared way
//! (Technical Plan P0.6) and records one startup span, printing its trace ID when spans are
//! exported, so its arrival in Grafana can be checked.

use std::process::ExitCode;

const SERVICE: &str = "voidforge-server";

fn main() -> ExitCode {
    let telemetry = match voidforge_telemetry::init(SERVICE) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let span = tracing::info_span!("stub_startup", binary = SERVICE);
    span.in_scope(|| tracing::info!("{SERVICE} stub started; no features yet"));
    if let Some(trace_id) = voidforge_telemetry::trace_id(&span) {
        println!("trace_id={trace_id}");
    }
    drop(span);
    match telemetry.shutdown() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            ExitCode::FAILURE
        }
    }
}
