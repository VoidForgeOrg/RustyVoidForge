//! Scripted players for end-to-end and nightly runs, plus a terminal client with device code login.
//!
//! P3.6 adds the first subcommand, `e2e`: the end-to-end scenario (decision log D-052), which
//! `.github/scripts/e2e.sh` runs against a fresh universe on the fast rules. The bots reach the
//! server over HTTP alone, through `voidforge-api-types`. Telemetry is initialized the shared way
//! (D-016).
//!
//! `e2e`'s exit codes: 0 passed; 1 a check failed; 2 usage error; 3 not ready, or a deadline
//! passed; 4 login, transport or protocol error; 5 telemetry failed.

mod client;
mod login;
mod scenario;
mod wait;

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

const SERVICE: &str = "voidforge-bots";

const E2E_EXIT_CODES: &str = "Exit codes: 0 passed, 1 a check failed, 2 usage error, 3 not ready \
    or a deadline passed, 4 login, transport or protocol error, 5 telemetry failed.";

/// Voidforge's scripted players.
#[derive(Debug, Parser)]
#[command(name = SERVICE, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// The end-to-end scenario: alice and bob log in through dex (the password grant, so the CI
    /// dex config), join, build, race to colonize one planet and transport cargo, checking every
    /// answer. Needs a fresh universe on the fast rules, with no other player.
    #[command(after_help = E2E_EXIT_CODES)]
    E2e {
        /// The API's base URL.
        #[arg(long, env = "VOIDFORGE_API", default_value = "http://127.0.0.1:8080")]
        api: String,
        /// dex's issuer URL.
        #[arg(
            long,
            env = "VOIDFORGE_ISSUER",
            default_value = "http://localhost:5556/dex"
        )]
        issuer: String,
        /// A tag unique to this run, in every idempotency key (1 to 64 bytes of printable ASCII,
        /// no spaces).
        #[arg(long, value_parser = run_tag)]
        run: String,
        /// The whole scenario's deadline, in seconds (1 to 3,600).
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3_600))]
        deadline_secs: u64,
    },
}

/// Why the scenario stopped.
#[derive(Debug)]
pub enum Failure {
    /// A check failed: the product answered something else.
    Assertion {
        /// The scenario's step.
        step: String,
        /// What was expected and what came back.
        detail: String,
    },
    /// A wait's deadline passed.
    Deadline {
        /// What was awaited.
        step: String,
        /// The last view.
        detail: String,
    },
    /// The server answered 503 `unavailable`: not ready.
    NotReady(String),
    /// dex refused a login.
    Login(String),
    /// The server or dex couldn't be reached, or answered outside the protocol.
    Http(String),
}

impl Failure {
    fn exit(&self) -> u8 {
        match self {
            Self::Assertion { .. } => 1,
            Self::Deadline { .. } | Self::NotReady(_) => 3,
            Self::Login(_) | Self::Http(_) => 4,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assertion { step, detail } => write!(f, "FAILED at {step}: {detail}"),
            Self::Deadline { step, detail } => {
                write!(f, "DEADLINE passed at {step}; the last view: {detail}")
            }
            Self::NotReady(detail) => write!(f, "NOT READY: {detail}"),
            Self::Login(detail) => write!(f, "LOGIN FAILED: {detail}"),
            Self::Http(detail) => write!(f, "HTTP ERROR: {detail}"),
        }
    }
}

/// A run tag: 1 to 64 bytes of printable ASCII without spaces, so every idempotency key stays
/// within the API's 128 bytes.
fn run_tag(text: &str) -> Result<String, String> {
    if !text.is_empty() && text.len() <= 64 && text.bytes().all(|b| b.is_ascii_graphic()) {
        Ok(text.to_owned())
    } else {
        Err("1 to 64 bytes of printable ASCII without spaces".to_owned())
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let telemetry = match voidforge_telemetry::init(SERVICE) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            return ExitCode::from(5);
        }
    };
    let code = match cli.command {
        Command::E2e {
            api,
            issuer,
            run,
            deadline_secs,
        } => {
            let span = tracing::info_span!("e2e", binary = SERVICE, run = %run);
            let code =
                span.in_scope(|| e2e(&api, &issuer, &run, Duration::from_secs(deadline_secs)));
            if let Some(trace_id) = voidforge_telemetry::trace_id(&span) {
                eprintln!("trace_id={trace_id}");
            }
            code
        }
    };
    match telemetry.shutdown() {
        Ok(()) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            // A failed shutdown is never a pass (D-016); a failure keeps its own code.
            ExitCode::from(if code == 0 { 5 } else { code })
        }
    }
}

fn e2e(api: &str, issuer: &str, run: &str, deadline: Duration) -> u8 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("can't start the runtime: {error}");
            return 4;
        }
    };
    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(http) => http,
        Err(error) => {
            tracing::error!("can't build the HTTP client: {error}");
            return 4;
        }
    };
    let started = std::time::Instant::now();
    let outcome = runtime.block_on(async {
        tokio::time::timeout(deadline, scenario::run(&http, api, issuer, run))
            .await
            .unwrap_or_else(|_| {
                Err(Failure::Deadline {
                    step: format!("the whole scenario ({} s)", deadline.as_secs()),
                    detail: "the last step logged above".to_owned(),
                })
            })
    });
    let secs = started.elapsed().as_secs_f64();
    match outcome {
        Ok(()) => {
            tracing::info!("e2e PASSED in {secs:.1} s");
            0
        }
        Err(failure) => {
            tracing::error!("e2e {failure} (after {secs:.1} s)");
            failure.exit()
        }
    }
}
