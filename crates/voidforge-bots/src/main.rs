//! Scripted players for end-to-end and nightly runs, plus a terminal client with device code login.
//!
//! P3.6 adds the first subcommand, `e2e`: the end-to-end scenario (decision log D-052), which
//! `.github/scripts/e2e.sh` runs against a fresh universe on the fast rules. The bots reach the
//! server over HTTP alone, through `voidforge-api-types`. Telemetry is initialized the shared way
//! (D-016).
//!
//! P3.7 adds the terminal (D-053): `login` through dex's device code flow, `logout`, and `me`,
//! `get` and `command` with the saved login.
//!
//! The exit codes are one table for every subcommand ([`EXIT_CODES`]): 0 done; 1 an e2e check
//! failed; 2 usage error; 3 not ready, or a deadline passed; 4 login, transport or protocol error;
//! 5 telemetry failed; 6 the login was declined; 7 the login code expired; 8 log in again (no
//! saved login, it expired, or it was refused); 9 the API refused the request.

mod client;
mod login;
mod scenario;
mod session;
mod terminal;
mod wait;

use std::path::PathBuf;

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

const SERVICE: &str = "voidforge-bots";

/// Every subcommand's exit codes: each code means one thing across the binary. A macro, so the
/// terminal's help can include it in one literal.
macro_rules! exit_codes {
    () => {
        "Exit codes: 0 done, 1 an e2e check failed, 2 usage error, 3 not ready or a deadline \
         passed, 4 login, transport or protocol error, 5 telemetry failed, 6 the login was \
         declined, 7 the login code expired, 8 log in again (no saved login, it expired, or it was \
         refused), 9 the API refused the request."
    };
}

const EXIT_CODES: &str = exit_codes!();

/// The terminal subcommands' help footer: where the login is saved, and the exit codes.
const TERMINAL_HELP: &str = concat!(
    "The saved login: $VOIDFORGE_TOKEN_FILE, else $XDG_CONFIG_HOME/voidforge/token.json, else \
     ~/.config/voidforge/token.json. It's user-only (0600), and refused when others can read it.\n\n",
    exit_codes!()
);

/// Voidforge's scripted players, and a terminal client.
#[derive(Debug, Parser)]
#[command(name = SERVICE, about, long_about = None, after_help = EXIT_CODES)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// For `just trace-check` (D-064): records one `telemetry_check` span, prints `trace_id=<hex>`
    /// on stdout when exporting, and exits; 5 if the span can't be delivered.
    #[command(hide = true)]
    TelemetryCheck,
    /// The end-to-end scenario: alice and bob log in through dex (the password grant, so the CI
    /// dex config), join, build, race to colonize one planet and transport cargo, checking every
    /// answer. Needs a fresh universe on the fast rules, with no other player.
    #[command(after_help = EXIT_CODES)]
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
    /// Logs in through dex's device code flow: shows a link and a code to approve in a browser,
    /// waits, then saves the login (valid for dex's 24 hours) for the other commands.
    #[command(after_help = TERMINAL_HELP)]
    Login {
        /// dex's issuer URL.
        #[arg(
            long,
            env = "VOIDFORGE_ISSUER",
            default_value = "http://localhost:5556/dex"
        )]
        issuer: String,
    },
    /// Deletes the saved login.
    #[command(after_help = TERMINAL_HELP)]
    Logout,
    /// Your player: GET /players/me, as JSON. The first call joins the game.
    #[command(after_help = TERMINAL_HELP)]
    Me {
        /// The API's base URL.
        #[arg(long, env = "VOIDFORGE_API", default_value = "http://127.0.0.1:8080")]
        api: String,
    },
    /// Any GET the API offers, by its path, as JSON: for example /players/me/fleets,
    /// /planets/27, '/planets?near=0,0,0&radius=100000' or /leaderboard.
    #[command(after_help = TERMINAL_HELP)]
    Get {
        /// The API's base URL.
        #[arg(long, env = "VOIDFORGE_API", default_value = "http://127.0.0.1:8080")]
        api: String,
        /// The API path, starting with /.
        path: String,
    },
    /// Sends one command: POST /commands with a JSON command, for example
    /// '{"type":"build_building","payload":{"planet_id":27,"building_type":"shipyard"}}'.
    #[command(name = "command", after_help = TERMINAL_HELP)]
    Send {
        /// The API's base URL.
        #[arg(long, env = "VOIDFORGE_API", default_value = "http://127.0.0.1:8080")]
        api: String,
        /// The command, as JSON.
        json: String,
        /// The idempotency key; a fresh one when omitted. The key used is printed, so a command
        /// can be retried exactly with --key.
        #[arg(long)]
        key: Option<String>,
    },
}

/// Why a subcommand stopped.
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
    /// A usage error the command line couldn't catch: JSON that isn't a command, a path that
    /// isn't an API path, a login that can't be saved.
    Usage(String),
    /// The person declined the login (`access_denied`).
    Declined(String),
    /// The login code expired before it was approved (`expired_token`, or `expires_in` passed).
    CodeExpired(String),
    /// No usable saved login: none, expired, readable by others, or refused by the API.
    LogInAgain(String),
    /// The API refused the request: its status, code and message.
    Refused(String),
}

impl Failure {
    fn exit(&self) -> u8 {
        match self {
            Self::Assertion { .. } => 1,
            Self::Deadline { .. } | Self::NotReady(_) => 3,
            Self::Login(_) | Self::Http(_) => 4,
            Self::Usage(_) => 2,
            Self::Declined(_) => 6,
            Self::CodeExpired(_) => 7,
            Self::LogInAgain(_) => 8,
            Self::Refused(_) => 9,
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
            Self::Usage(detail)
            | Self::Declined(detail)
            | Self::CodeExpired(detail)
            | Self::LogInAgain(detail)
            | Self::Refused(detail) => f.write_str(detail),
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
    let code =
        match cli.command {
            Command::TelemetryCheck => {
                let span = tracing::info_span!("telemetry_check", binary = SERVICE);
                span.in_scope(|| tracing::info!("telemetry check"));
                if let Some(trace_id) = voidforge_telemetry::trace_id(&span) {
                    println!("trace_id={trace_id}");
                }
                0
            }
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
            Command::Login { issuer } => {
                terminal(|http, file| async move { terminal::login(&http, &issuer, &file).await })
            }
            Command::Logout => terminal(|_, file| async move { terminal::logout(&file) }),
            Command::Me { api } => {
                terminal(|http, file| async move { terminal::me(&http, &api, &file).await })
            }
            Command::Get { api, path } => {
                terminal(|http, file| async move { terminal::get(&http, &api, &file, &path).await })
            }
            Command::Send { api, json, key } => terminal(|http, file| async move {
                terminal::command(&http, &api, &file, &json, key).await
            }),
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

/// Runs one terminal subcommand on a runtime with an HTTP client and the saved login's path,
/// printing a failure for the person on stderr.
fn terminal<F, Fut>(run: F) -> u8
where
    F: FnOnce(reqwest::Client, PathBuf) -> Fut,
    Fut: Future<Output = Result<(), Failure>>,
{
    let outcome = (|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Failure::Http(format!("can't start the runtime: {e}")))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Failure::Http(format!("can't build the HTTP client: {e}")))?;
        let file = session::path()?;
        runtime.block_on(run(http, file))
    })();
    match outcome {
        Ok(()) => 0,
        Err(failure) => {
            eprintln!("{SERVICE}: {failure}");
            failure.exit()
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
