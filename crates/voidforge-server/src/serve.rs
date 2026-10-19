//! `voidforge-server serve`: run the API for real (Technical Plan P3.4s; decision log D-050).
//!
//! Startup:
//! 1. the token-check settings ([`AuthArgs`]) and the rules file are checked;
//! 2. [`voidforge_store::open_universe`] refuses a database without a universe *before* migrating
//!    it, then migrates and reads the recorded epoch: never one from a flag or the clock;
//! 3. [`voidforge_host::recover`] restores the newest snapshot and replays the tail (D-038), and
//!    [`voidforge_host::spawn`] starts the host over Postgres on the system clock;
//! 4. `refresh_forever` keeps dex's keys fresh beside the server (D-046): with dex down the server
//!    still starts, and tokens get 503 until the keys load;
//! 5. the listener binds, and one `ready:` line goes to stdout.
//!
//! Shutdown, on the first SIGINT or SIGTERM, or when the host stops on its own:
//! 1. stop accepting, and drain in-flight requests for at most `--drain-secs`;
//! 2. shut the runtime down: the router, its `AppState` and every connection the drain cut off go
//!    with it, and so does every [`HostHandle`]. Nothing else holds one, so the host stops and
//!    writes its clean-shutdown snapshot (D-038);
//! 3. join the host, bounded: a handle that survived would otherwise block forever;
//! 4. read Postgres: exit 0 only if the newest snapshot is at the log head. `HostTask::join` doesn't
//!    report a failed shutdown snapshot, and the database is the truth anyway.
//!
//! A second signal exits at once. Every unclean stop is exit 6, and recovery then replays the log
//! from the last snapshot: every acknowledged command's record is durable (D-036).

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::signal::unix::{Signal, SignalKind, signal};
use voidforge_host::{Clock, HostConfig, HostHandle, HostTask, RecoverError, SystemClock};
use voidforge_store::OpenError;
use voidforge_store::accounts::PgAccounts;
use voidforge_store::log::{LogStore, PgLog};
use voidforge_store::replay::ReplayError;
use voidforge_store::snapshot::{ENGINE_BUILD, EngineBuild, SnapshotError};

use crate::accounts::Players;
use crate::api::{self, AppState};
use crate::auth::{AuthArgs, TokenVerifier};

/// The default bind address: loopback, so a local run is never exposed by accident.
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// The longest drain `--drain-secs` accepts.
pub const MAX_DRAIN_SECS: u64 = 60;

/// How long the host may take to stop and write its snapshot once every handle is gone.
pub const JOIN_BOUND: Duration = Duration::from_secs(10);

/// What `serve` takes. The database URL is required and has no default, and it is deliberately
/// not `DATABASE_URL`: a stray one, set for sqlx's tooling, could point here at the owner's
/// database (D-050).
#[derive(Debug, Clone, clap::Args)]
pub struct ServeArgs {
    /// The universe's database. Required; never printed.
    #[arg(long, env = "VOIDFORGE_DATABASE_URL", hide_env_values = true)]
    pub database_url: String,
    /// The rules file; its version must be the universe's (for example rules/v0.toml).
    #[arg(long, env = "VOIDFORGE_RULES")]
    pub rules: PathBuf,
    /// The address to listen on. Port 0 picks a free port; the `ready:` line names it.
    #[arg(long, env = "VOIDFORGE_BIND", default_value = DEFAULT_BIND)]
    pub bind: SocketAddr,
    /// How long in-flight requests may finish after a stop signal, in seconds (0 to 60).
    #[arg(
        long,
        env = "VOIDFORGE_DRAIN_SECS",
        default_value_t = 5,
        value_parser = clap::value_parser!(u64).range(0..=MAX_DRAIN_SECS)
    )]
    pub drain_secs: u64,
    /// The token checks (D-046).
    #[command(flatten)]
    pub auth: AuthArgs,
}

/// How `serve` ended: its exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeExit {
    /// Stopped cleanly: the final snapshot is at the log head.
    Clean = 0,
    /// A setting out of range (clap's own usage errors exit 2 too).
    Usage = 2,
    /// No universe, no recorded epoch, an epoch out of range, or another rules version.
    Refused = 3,
    /// The database, the rules file, recovery or the bind failed.
    Input = 4,
    /// A second signal, a drain timeout, the host stopping on its own or not stopping, or no
    /// snapshot at the head.
    Unclean = 6,
}

/// Why serving stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// A first SIGINT or SIGTERM: the clean path.
    Signal(&'static str),
    /// The host stopped on its own (a failed append, an engine error).
    HostStopped,
    /// The server itself ended, which it never should.
    ServerEnded,
}

/// Runs `serve` to the end.
#[must_use]
pub fn run(args: &ServeArgs) -> ServeExit {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return fail(ServeExit::Input, &format!("runtime: {e}")),
    };
    let served = runtime.block_on(serve(args));
    // Drops everything the runtime still runs: the key refresher, and any connection the drain
    // cut off, with the host handle its request holds.
    runtime.shutdown_timeout(Duration::from_secs(1));
    let (task, stop, drained) = match served {
        Ok(Served::Stopped {
            task,
            stop,
            drained,
        }) => (task, stop, drained),
        Ok(Served::SecondSignal) => {
            tracing::error!(
                "second signal: stopping without a final snapshot; recovery replays the log from the last snapshot"
            );
            return ServeExit::Unclean;
        }
        Err(exit) => return exit,
    };
    let stopped = stop_host(task, &args.database_url);
    match (stop, drained, stopped) {
        (Stop::Signal(_), true, ServeExit::Clean) => {
            tracing::info!("stopped cleanly; the final snapshot is at the log head");
            ServeExit::Clean
        }
        (_, _, ServeExit::Clean) => {
            let why = match stop {
                Stop::HostStopped => "the host stopped on its own",
                Stop::ServerEnded => "the server ended on its own",
                Stop::Signal(_) => "requests were still in flight when the drain ended",
            };
            fail(ServeExit::Unclean, &format!("stopped uncleanly: {why}"))
        }
        (_, _, exit) => exit,
    }
}

enum Served {
    Stopped {
        task: HostTask,
        stop: Stop,
        /// Whether every in-flight request finished within the drain.
        drained: bool,
    },
    SecondSignal,
}

/// Logs the failure, puts it on stderr, and returns its exit.
fn fail(exit: ServeExit, message: &str) -> ServeExit {
    tracing::error!(message);
    let label = match exit {
        ServeExit::Refused => "REFUSED",
        _ => "ERROR",
    };
    eprintln!("voidforge-server serve: {label}: {message}");
    exit
}

/// SIGINT or SIGTERM, whichever comes first.
struct Signals {
    interrupt: Signal,
    terminate: Signal,
}

impl Signals {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn next(&mut self) -> &'static str {
        tokio::select! {
            _ = self.interrupt.recv() => "SIGINT",
            _ = self.terminate.recv() => "SIGTERM",
        }
    }
}

/// Starts, serves until a signal or the host stops, and drains.
async fn serve(args: &ServeArgs) -> Result<Served, ServeExit> {
    // Signals first: one that arrives while recovering is handled once serving starts.
    let mut signals =
        Signals::new().map_err(|e| fail(ServeExit::Input, &format!("signal handlers: {e}")))?;
    let auth = args
        .auth
        .config()
        .map_err(|e| fail(ServeExit::Usage, &e.to_string()))?;
    let rules_path = args.rules.display();
    let text = std::fs::read_to_string(&args.rules)
        .map_err(|e| fail(ServeExit::Input, &format!("{rules_path}: {e}")))?;
    let rules = voidforge_rules::parse(&text)
        .map_err(|e| fail(ServeExit::Input, &format!("{rules_path}: {e}")))?;
    let url = args.database_url.as_str();
    let epoch = voidforge_store::open_universe(url)
        .await
        .map_err(|e| match e {
            OpenError::NoUniverse | OpenError::NoEpoch | OpenError::EpochOutOfRange(_) => {
                fail(ServeExit::Refused, &e.to_string())
            }
            // The URL holds a password: the error never echoes it.
            OpenError::Url => fail(ServeExit::Input, "--database-url is not a Postgres URL"),
            other => fail(ServeExit::Input, &other.to_string()),
        })?;
    let build =
        EngineBuild::new(ENGINE_BUILD).map_err(|e| fail(ServeExit::Input, &e.to_string()))?;
    let mut log = PgLog::lazy_url(url).map_err(|e| fail(ServeExit::Input, &e.to_string()))?;
    let recovered = voidforge_host::recover(&mut log, &rules, &build)
        .await
        .map_err(|e| match e {
            RecoverError::Snapshot {
                error: SnapshotError::RulesMismatch { .. },
                ..
            }
            | RecoverError::Replay(ReplayError::RulesMismatch { .. }) => {
                fail(ServeExit::Refused, &format!("recovery: {e}"))
            }
            RecoverError::NoSnapshot => fail(ServeExit::Refused, &e.to_string()),
            other => fail(ServeExit::Input, &format!("recovery failed: {other}")),
        })?;
    drop(log);
    tracing::info!(
        build = ENGINE_BUILD,
        rules_version = rules.version,
        epoch_unix_ms = epoch,
        snapshot_seq = recovered.snapshot_seq,
        last_seq = recovered.last_seq,
        "recovered the universe"
    );
    let store_url = url.to_owned();
    let config = HostConfig::new(epoch, build);
    // `spawn` waits for the host to start: a blocking wait, kept off the async workers.
    let (host, task) = tokio::task::block_in_place(|| {
        voidforge_host::spawn(
            recovered.world,
            rules,
            // The URL parsed above, so this can't fail.
            move || PgLog::lazy_url(&store_url).expect("the URL is a Postgres URL"),
            SystemClock,
            config,
        )
    })
    .map_err(|e| fail(ServeExit::Input, &format!("the host could not start: {e}")))?;
    let accounts = PgAccounts::lazy_url(url).expect("the URL is a Postgres URL");
    let verifier = Arc::new(
        TokenVerifier::new(auth.clone())
            .map_err(|e| fail(ServeExit::Input, &format!("token checks: {e}")))?,
    );
    tokio::spawn(verifier.clone().refresh_forever());
    tracing::info!(
        issuer = %auth.issuer,
        "dex's keys load in the background; until they do, token checks answer 503"
    );
    // The one handle `serve` keeps, to notice the host stopping; dropped before the drain.
    let mut watch = host.clone();
    let seq = host.read().seq;
    let state = AppState {
        verifier,
        players: Arc::new(Players::new(accounts, host.clone())),
        host,
        now_unix_ms: Arc::new(|| SystemClock.now_unix_ms()),
        epoch_unix_ms: epoch,
    };
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .map_err(|e| fail(ServeExit::Input, &format!("bind {}: {e}", args.bind)))?;
    let addr = listener
        .local_addr()
        .map_err(|e| fail(ServeExit::Input, &format!("bind {}: {e}", args.bind)))?;
    let (drain, drain_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, api::router(state))
            .with_graceful_shutdown(async {
                let _ = drain_rx.await;
            })
            .await
    });
    let epoch_text = api::time::timestamp(epoch, 0)
        .ok()
        .and_then(|t| serde_json::to_value(t).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| epoch.to_string());
    tracing::info!(%addr, seq, "serving");
    println!("ready: listening on {addr}, epoch {epoch_text}, seq {seq}");
    let _ = std::io::stdout().flush();

    let stop = tokio::select! {
        name = signals.next() => Stop::Signal(name),
        () = host_stopped(&mut watch) => Stop::HostStopped,
        _ = &mut server => Stop::ServerEnded,
    };
    drop(watch);
    let drain_for = Duration::from_secs(args.drain_secs);
    match stop {
        Stop::Signal(name) => {
            tracing::info!(signal = name, drain_secs = args.drain_secs, "shutting down");
        }
        Stop::HostStopped => {
            tracing::error!("the host stopped on its own; shutting down so a restart recovers");
        }
        Stop::ServerEnded => tracing::error!("the server ended on its own; shutting down"),
    }
    let _ = drain.send(());
    let drained = tokio::select! {
        ended = tokio::time::timeout(drain_for, &mut server) => ended.is_ok(),
        name = signals.next() => {
            tracing::error!(signal = name, "second signal while draining");
            return Ok(Served::SecondSignal);
        }
    };
    if !drained {
        tracing::error!(
            drain_secs = args.drain_secs,
            "requests were still in flight when the drain ended; cutting them off"
        );
    }
    Ok(Served::Stopped {
        task,
        stop,
        drained,
    })
}

/// Resolves once the host has stopped: its read model can't change any more.
async fn host_stopped(handle: &mut HostHandle) {
    while handle.changed().await.is_ok() {}
}

/// Joins the host within [`JOIN_BOUND`], then checks the final snapshot in Postgres. Clean only
/// if the host stopped without a fatal error and the newest snapshot is at the log head.
fn stop_host(task: HostTask, url: &str) -> ServeExit {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return fail(ServeExit::Unclean, &format!("runtime: {e}")),
    };
    let exit = runtime.block_on(async {
        let mut signals = Signals::new().ok();
        let second = async {
            match signals.as_mut() {
                Some(s) => s.next().await,
                None => std::future::pending().await,
            }
        };
        let joined = tokio::task::spawn_blocking(move || task.join());
        tokio::select! {
            joined = tokio::time::timeout(JOIN_BOUND, joined) => match joined {
                Err(_) => fail(
                    ServeExit::Unclean,
                    "the host did not stop within 10 s: something still holds a host handle",
                ),
                Ok(Err(e)) => fail(ServeExit::Unclean, &format!("the host thread: {e}")),
                Ok(Ok(Err(fatal))) => fail(ServeExit::Unclean, &format!("the host stopped: {fatal}")),
                Ok(Ok(Ok(()))) => snapshot_at_head(url).await,
            },
            name = second => {
                tracing::error!(signal = name, "second signal while the host stops");
                ServeExit::Unclean
            }
        }
    });
    runtime.shutdown_timeout(Duration::ZERO);
    exit
}

/// Whether the newest snapshot in Postgres is at the log head.
async fn snapshot_at_head(url: &str) -> ServeExit {
    let Ok(mut log) = PgLog::lazy_url(url) else {
        return fail(ServeExit::Unclean, "--database-url is not a Postgres URL");
    };
    let snapshot = match log.latest_snapshot(None).await {
        Ok(row) => row.map(|r| r.seq),
        Err(e) => return fail(ServeExit::Unclean, &format!("reading the snapshots: {e}")),
    };
    let head = match log.last().await {
        Ok(last) => last.map_or(0, |(seq, _)| seq),
        Err(e) => return fail(ServeExit::Unclean, &format!("reading the log head: {e}")),
    };
    if snapshot == Some(head) {
        tracing::info!(seq = head, "the final snapshot is at the log head");
        ServeExit::Clean
    } else {
        fail(
            ServeExit::Unclean,
            &format!(
                "the newest snapshot ({}) is not at the log head ({head}); recovery replays the tail",
                snapshot.map_or_else(|| "none".to_owned(), |s| s.to_string())
            ),
        )
    }
}
