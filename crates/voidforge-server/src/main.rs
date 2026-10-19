//! API, token checks, accounts and the leaderboard job.
//!
//! P2.9 adds the first subcommand, `seed`: the universe bootstrap (decision log D-043). It
//! generates a universe from a seed (`voidforge_worldgen::generate`, option A), encodes it as a
//! snapshot under this binary's build, and writes it at seq 0 into an empty database, after
//! applying the migrations, with the universe's epoch beside it (D-050). It never overwrites a
//! universe. P3.4s adds `serve` (`voidforge_server::serve`), which runs the API over it.
//!
//! `seed`'s exit codes (as the replay tool's, D-041): 0 seeded; 2 usage error; 3 refused (the
//! database is not empty, the universe is too large, or the epoch is out of range); 4 input or
//! database error; 5 telemetry failed. `serve`'s are in [`SERVE_EXIT_CODES`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde::Serialize;
use voidforge_server::serve::{self, ServeArgs, ServeExit};
use voidforge_store::BootstrapError;
use voidforge_store::snapshot::{self, ENGINE_BUILD, EngineBuild};
use voidforge_store::verify::hex;
use voidforge_worldgen::WorldgenError;

const SERVICE: &str = "voidforge-server";

const SEED_EXIT_CODES: &str = "Exit codes: 0 seeded, 2 usage error, 3 refused (the database is \
    not empty, the universe is too large, or the epoch is out of range), 4 input or database \
    error, 5 telemetry failed.";

const SERVE_EXIT_CODES: &str = "Exit codes: 0 stopped cleanly (the final snapshot is at the log \
    head), 2 usage error (a setting out of range), 3 refused (no universe, no recorded epoch, an \
    epoch out of range, or another rules version), 4 input or database error (connecting, \
    migrating, recovery, or the bind), 5 telemetry failed, 6 stopped uncleanly (a second signal, \
    a drain timeout, or the host stopped on its own; recovery replays the log).";

/// The Voidforge server.
#[derive(Debug, Parser)]
#[command(name = SERVICE, version = ENGINE_BUILD, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bootstraps a universe: generates it from a seed and writes its first snapshot (seq 0) and
    /// its epoch into an empty database. Never overwrites a universe.
    #[command(after_help = SEED_EXIT_CODES)]
    Seed {
        /// The universe seed.
        #[arg(long)]
        seed: u64,
        /// Expected players: one homeworld candidate each.
        #[arg(long)]
        players: u32,
        /// The rules file (for example rules/v0.toml).
        #[arg(long)]
        rules: PathBuf,
        /// The database (default: DATABASE_URL). It is never printed.
        #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
        database_url: String,
        /// The universe's epoch, Unix milliseconds: wall time at game time 0, from 1970-01-01 up
        /// to 2100-01-01 (default: now). Recorded once; `serve` reads it.
        #[arg(long, allow_hyphen_values = true)]
        epoch_unix_ms: Option<i64>,
        /// One JSON document on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Serves the API over the universe: recovers the host from Postgres at the recorded epoch,
    /// and on SIGINT or SIGTERM drains and stops with a final snapshot.
    #[command(after_help = SERVE_EXIT_CODES)]
    Serve(ServeArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    Seeded = 0,
    Refused = 3,
    Input = 4,
    Telemetry = 5,
}

#[derive(Debug, Serialize)]
struct Seeded {
    result: &'static str,
    seed: u64,
    players: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    planets: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rules_version: Option<u32>,
    build: String,
    /// The epoch, Unix milliseconds, and as an RFC 3339 time.
    epoch_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_hash: Option<String>,
    /// On a refusal because the database holds a universe: its seq-0 state hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    existing_state_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let telemetry = match voidforge_telemetry::init(SERVICE) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            return ExitCode::from(Exit::Telemetry as u8);
        }
    };
    let (span, exit) = match cli.command {
        Command::Seed {
            seed,
            players,
            rules,
            database_url,
            epoch_unix_ms,
            json,
        } => {
            let span = tracing::info_span!("seed", binary = SERVICE);
            let exit = span.in_scope(|| {
                seed_command(seed, players, &rules, &database_url, epoch_unix_ms, json)
            });
            (span, exit as u8)
        }
        Command::Serve(args) => {
            let span = tracing::info_span!("serve", binary = SERVICE);
            let exit = span.in_scope(|| serve::run(&args));
            (span, exit as u8)
        }
    };
    if let Some(trace_id) = voidforge_telemetry::trace_id(&span) {
        eprintln!("trace_id={trace_id}");
    }
    drop(span);
    match telemetry.shutdown() {
        Ok(()) => ExitCode::from(exit),
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            // A failed shutdown is never a success (D-016); a failure keeps its own code.
            let code = if exit == Exit::Seeded as u8 || exit == ServeExit::Clean as u8 {
                Exit::Telemetry as u8
            } else {
                exit
            };
            ExitCode::from(code)
        }
    }
}

/// Now, in Unix milliseconds: the default epoch, read once.
fn now_unix_ms() -> Result<i64, String> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "the system clock is before 1970".to_owned())?;
    i64::try_from(since.as_millis()).map_err(|_| "the system clock is beyond i64 ms".to_owned())
}

/// The epoch as an RFC 3339 time, when it is one.
fn epoch_text(epoch_unix_ms: i64) -> Option<String> {
    let t = voidforge_server::api::time::timestamp(epoch_unix_ms, 0).ok()?;
    serde_json::to_value(t).ok()?.as_str().map(str::to_owned)
}

fn seed_command(
    seed: u64,
    players: u32,
    rules: &Path,
    url: &str,
    epoch_unix_ms: Option<i64>,
    json: bool,
) -> Exit {
    let epoch_unix_ms = match epoch_unix_ms.map_or_else(now_unix_ms, Ok) {
        Ok(epoch) => epoch,
        Err(message) => {
            eprintln!("{SERVICE}: {message}");
            return Exit::Input;
        }
    };
    let mut out = Seeded {
        result: "seeded",
        seed,
        players,
        planets: None,
        rules_version: None,
        build: ENGINE_BUILD.to_owned(),
        epoch_unix_ms,
        epoch: epoch_text(epoch_unix_ms),
        state_hash: None,
        existing_state_hash: None,
        error: None,
    };
    let exit = match bootstrap(seed, players, rules, url, epoch_unix_ms, &mut out) {
        Ok(()) => Exit::Seeded,
        Err((exit, message)) => {
            out.result = if exit == Exit::Refused {
                "refused"
            } else {
                "error"
            };
            out.error = Some(message);
            exit
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&out).expect("output serializes")
        );
    } else if exit == Exit::Seeded {
        println!(
            "seeded: seed {seed}, players {players}, planets {}, rules v{}, build {}, epoch {} ({} ms), state hash {}",
            out.planets.unwrap_or_default(),
            out.rules_version.unwrap_or_default(),
            out.build,
            out.epoch.as_deref().unwrap_or("?"),
            out.epoch_unix_ms,
            out.state_hash.as_deref().unwrap_or("?"),
        );
    } else {
        println!(
            "{}: {}{}",
            out.result.to_uppercase(),
            out.error.as_deref().unwrap_or("?"),
            out.existing_state_hash
                .as_deref()
                .map_or_else(String::new, |h| format!(" (its seq-0 state hash is {h})")),
        );
    }
    exit
}

fn bootstrap(
    seed: u64,
    players: u32,
    rules_path: &Path,
    url: &str,
    epoch_unix_ms: i64,
    out: &mut Seeded,
) -> Result<(), (Exit, String)> {
    let input = |m: String| (Exit::Input, m);
    // Refused before anything is read, generated or connected (D-050).
    if !voidforge_store::epoch_in_window(epoch_unix_ms) {
        return Err((
            Exit::Refused,
            BootstrapError::EpochOutOfRange(epoch_unix_ms).to_string(),
        ));
    }
    let text = std::fs::read_to_string(rules_path)
        .map_err(|e| input(format!("{}: {e}", rules_path.display())))?;
    let rules = voidforge_rules::parse(&text)
        .map_err(|e| input(format!("{}: {e}", rules_path.display())))?;
    out.rules_version = Some(rules.version);
    let world = voidforge_worldgen::generate(seed, players, &rules).map_err(|e| match e {
        WorldgenError::TooLarge { .. } | WorldgenError::NoPlayers => (Exit::Refused, e.to_string()),
        other => input(format!("world generation: {other}")),
    })?;
    out.planets = Some(world.planets.len());
    let build = EngineBuild::new(ENGINE_BUILD).map_err(|e| input(e.to_string()))?;
    let snap = snapshot::encode(&world, &build).map_err(|e| input(e.to_string()))?;
    let state_hash = hex(&snap.header.state_hash);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| input(format!("runtime: {e}")))?;
    runtime
        .block_on(voidforge_store::bootstrap(url, &snap, epoch_unix_ms))
        .map_err(|e| match e {
            BootstrapError::NotEmpty { seq0_state_hash } => {
                out.existing_state_hash = seq0_state_hash.as_deref().map(hex);
                (
                    Exit::Refused,
                    "the database already holds a universe; it is never overwritten".to_owned(),
                )
            }
            BootstrapError::HasAccounts { .. } | BootstrapError::EpochOutOfRange(_) => {
                (Exit::Refused, e.to_string())
            }
            // The URL holds a password: the error never echoes it.
            BootstrapError::Url => input("--database-url is not a Postgres URL".to_owned()),
            other => input(other.to_string()),
        })?;
    out.state_hash = Some(state_hash);
    Ok(())
}
