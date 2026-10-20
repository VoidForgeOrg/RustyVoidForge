//! `voidforge-server serve`: run the API for real (Technical Plan P3.4s; decision log D-050).
//!
//! Two runtimes. The **control** runtime (current-thread) lives for the whole process: it owns
//! the one signal stream, the universe's lock, and every step of the shutdown. The **server**
//! runtime (multi-thread) runs the API, its connections and the key refresher.
//!
//! Startup:
//! 1. the token-check settings ([`AuthArgs`]) and the rules file are checked;
//! 2. [`voidforge_store::open_universe`] refuses a database without a universe *before* migrating
//!    it, then migrates and reads the recorded epoch: never one from a flag or the clock;
//! 3. [`voidforge_store::lock_universe`] takes the universe's advisory lock, so a second `serve` on
//!    it refuses (exit 3) before recovering or binding;
//! 4. [`voidforge_host::recover`] restores the newest snapshot and replays the tail (D-038), and
//!    [`voidforge_host::spawn`] starts the host over Postgres on the system clock;
//! 5. `refresh_forever` keeps dex's keys fresh beside the server (D-046): with dex down the server
//!    still starts, and tokens get 503 until the keys load;
//! 6. the listener binds, and one `ready:` line goes to stdout.
//!
//! Shutdown, on the first SIGINT or SIGTERM, or when the host stops on its own:
//! 1. stop accepting, and drain in-flight requests for at most `--drain-secs`;
//! 2. shut the server runtime down: the router, its `AppState` and every connection the drain cut
//!    off go with it, and so does every [`HostHandle`]. Nothing else holds one, so the host stops
//!    and writes its clean-shutdown snapshot (D-038);
//! 3. join the host, bounded at [`JOIN_BOUND`]: a handle that survived would otherwise block
//!    forever;
//! 4. read Postgres, bounded at [`CHECK_BOUND`]: exit 0 only if the newest snapshot is at the log
//!    head. `HostTask::join` doesn't report a failed shutdown snapshot, and the database is the
//!    truth anyway.
//!
//! Every step, from the first signal to the exit, is selected against the same signal stream, so a
//! second signal at any moment exits at once (review-r2's M1 and QA's D1 on 6707d85: two runtimes'
//! streams once left gaps between and after the phases). Every unclean stop is exit 6, and recovery
//! then replays the log from the last snapshot: every acknowledged command's record is durable
//! (D-036).

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::signal::unix::{Signal, SignalKind, signal};
use voidforge_host::{Clock, HostConfig, HostHandle, HostTask, RecoverError, Rules, SystemClock};
use voidforge_store::accounts::PgAccounts;
use voidforge_store::log::{LogStore, PgLog};
use voidforge_store::replay::ReplayError;
use voidforge_store::snapshot::{ENGINE_BUILD, EngineBuild, SnapshotError};
use voidforge_store::{LockError, OpenError};

use crate::accounts::Players;
use crate::api::{self, AppState};
use crate::auth::{AuthArgs, TokenVerifier};
use crate::leaderboard;

/// The default bind address: loopback, so a local run is never exposed by accident.
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// The longest interval `--leaderboard-secs` accepts.
pub const MAX_LEADERBOARD_SECS: u64 = 3_600;

/// The longest drain `--drain-secs` accepts.
pub const MAX_DRAIN_SECS: u64 = 60;

/// How long the host may take to stop and write its snapshot once every handle is gone.
pub const JOIN_BOUND: Duration = Duration::from_secs(10);

/// How long the final check of the snapshot in Postgres may take (review-r2's MEDIUM: every step
/// of the shutdown is bounded and watches for a second signal).
pub const CHECK_BOUND: Duration = Duration::from_secs(5);

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
    /// How often the leaderboard is rebuilt, in seconds (1 to 3,600; the Game Design's 60).
    #[arg(
        long,
        env = "VOIDFORGE_LEADERBOARD_SECS",
        default_value_t = 60,
        value_parser = clap::value_parser!(u64).range(1..=MAX_LEADERBOARD_SECS)
    )]
    pub leaderboard_secs: u64,
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
    // The control runtime lives for the whole process: it owns the one signal stream, the
    // universe's lock, and every step of the shutdown, so no signal ever falls between phases.
    let control = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return fail(ServeExit::Input, &format!("runtime: {e}")),
    };
    let exit = control.block_on(serve(args));
    control.shutdown_timeout(Duration::ZERO);
    exit
}

/// The server's runtime: the API, its connections and the key refresher. Shutting it down drops
/// every connection the drain cut off, with the host handle its request holds. Dropping it shuts it
/// down in the background, which is allowed from async code, so no exit path blocks or panics.
struct ServerRuntime(Option<tokio::runtime::Runtime>);

impl ServerRuntime {
    fn handle(&self) -> tokio::runtime::Handle {
        self.0.as_ref().expect("running").handle().clone()
    }
}

impl Drop for ServerRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// What the server runtime hands back once it serves.
struct Started {
    task: HostTask,
    /// The one handle `serve` keeps, to notice the host stopping; dropped before the drain.
    watch: HostHandle,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
    drain: tokio::sync::oneshot::Sender<()>,
    addr: SocketAddr,
    seq: i64,
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

/// A second signal: stop at once, with exit 6.
fn second_signal(name: &str, during: &str) -> ServeExit {
    fail(
        ServeExit::Unclean,
        &format!(
            "second signal ({name}) {during}: stopping without waiting; recovery replays the log from the last snapshot"
        ),
    )
}

/// The whole life of `serve`, on the control runtime.
async fn serve(args: &ServeArgs) -> ServeExit {
    // Signals first: one that arrives while starting is handled once serving starts.
    let mut signals = match Signals::new() {
        Ok(signals) => signals,
        Err(e) => return fail(ServeExit::Input, &format!("signal handlers: {e}")),
    };
    let auth = match args.auth.config() {
        Ok(auth) => auth,
        Err(e) => return fail(ServeExit::Usage, &e.to_string()),
    };
    let rules_path = args.rules.display();
    let rules = match std::fs::read_to_string(&args.rules)
        .map_err(|e| e.to_string())
        .and_then(|text| voidforge_rules::parse(&text).map_err(|e| e.to_string()))
    {
        Ok(rules) => rules,
        Err(e) => return fail(ServeExit::Input, &format!("{rules_path}: {e}")),
    };
    let url = args.database_url.as_str();
    let epoch = match voidforge_store::open_universe(url).await {
        Ok(epoch) => epoch,
        Err(e @ (OpenError::NoUniverse | OpenError::NoEpoch | OpenError::EpochOutOfRange(_))) => {
            return fail(ServeExit::Refused, &e.to_string());
        }
        // The URL holds a password: the error never echoes it.
        Err(OpenError::Url) => {
            return fail(ServeExit::Input, "--database-url is not a Postgres URL");
        }
        Err(other) => return fail(ServeExit::Input, &other.to_string()),
    };
    // One serve per universe, before recovering or binding (QA's I4): held until this returns.
    let _lock = match voidforge_store::lock_universe(url).await {
        Ok(lock) => lock,
        Err(e @ LockError::Held) => return fail(ServeExit::Refused, &e.to_string()),
        Err(LockError::Url) => {
            return fail(ServeExit::Input, "--database-url is not a Postgres URL");
        }
        Err(e) => return fail(ServeExit::Input, &e.to_string()),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => ServerRuntime(Some(runtime)),
        Err(e) => return fail(ServeExit::Input, &format!("runtime: {e}")),
    };
    let start = runtime.handle().spawn(start(
        url.to_owned(),
        args.bind,
        rules,
        auth,
        epoch,
        Duration::from_secs(args.leaderboard_secs),
    ));
    let Started {
        task,
        mut watch,
        mut server,
        drain,
        addr,
        seq,
    } = match start.await {
        Ok(Ok(started)) => started,
        Ok(Err(exit)) => return exit,
        Err(e) => return fail(ServeExit::Input, &format!("starting: {e}")),
    };
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
    match stop {
        Stop::Signal(name) => {
            tracing::info!(signal = name, drain_secs = args.drain_secs, "shutting down");
        }
        Stop::HostStopped => {
            tracing::error!("the host stopped on its own; shutting down so a restart recovers");
        }
        Stop::ServerEnded => tracing::error!("the server ended on its own; shutting down"),
    }
    // 1. Drain.
    let _ = drain.send(());
    let drain_for = Duration::from_secs(args.drain_secs);
    let drained = tokio::select! {
        ended = tokio::time::timeout(drain_for, &mut server) => ended.is_ok(),
        name = signals.next() => return second_signal(name, "while draining"),
    };
    if !drained {
        tracing::error!(
            drain_secs = args.drain_secs,
            "requests were still in flight when the drain ended; cutting them off"
        );
    }
    // 2. The server runtime goes, and with it every connection and its host handle.
    drop(server);
    drop(runtime);
    // 3. The host stops once the last handle is gone, and writes its snapshot.
    let joined = tokio::task::spawn_blocking(move || task.join());
    let stopped = tokio::select! {
        joined = tokio::time::timeout(JOIN_BOUND, joined) => match joined {
            Err(_) => fail(
                ServeExit::Unclean,
                "the host did not stop within 10 s: something still holds a host handle",
            ),
            Ok(Err(e)) => fail(ServeExit::Unclean, &format!("the host thread: {e}")),
            Ok(Ok(Err(fatal))) => fail(ServeExit::Unclean, &format!("the host stopped: {fatal}")),
            Ok(Ok(Ok(()))) => ServeExit::Clean,
        },
        name = signals.next() => return second_signal(name, "while the host stops"),
    };
    // 4. Postgres says whether the final snapshot is durable.
    let stopped = match stopped {
        ServeExit::Clean => tokio::select! {
            exit = tokio::time::timeout(CHECK_BOUND, snapshot_at_head(url)) => exit.unwrap_or_else(|_| {
                fail(ServeExit::Unclean, "the final snapshot check did not finish within 5 s")
            }),
            name = signals.next() => return second_signal(name, "while checking the snapshot"),
        },
        exit => exit,
    };
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

/// On the server runtime: recovers, starts the host, the key refresher and the API.
async fn start(
    url: String,
    bind: SocketAddr,
    rules: Rules,
    auth: crate::auth::AuthConfig,
    epoch: i64,
    leaderboard_every: Duration,
) -> Result<Started, ServeExit> {
    let build =
        EngineBuild::new(ENGINE_BUILD).map_err(|e| fail(ServeExit::Input, &e.to_string()))?;
    let mut log = PgLog::lazy_url(&url).map_err(|e| fail(ServeExit::Input, &e.to_string()))?;
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
    let store_url = url.clone();
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
    let accounts = PgAccounts::lazy_url(&url).expect("the URL is a Postgres URL");
    let verifier = Arc::new(
        TokenVerifier::new(auth.clone())
            .map_err(|e| fail(ServeExit::Input, &format!("token checks: {e}")))?,
    );
    tokio::spawn(verifier.clone().refresh_forever());
    tracing::info!(
        issuer = %auth.issuer,
        "dex's keys load in the background; until they do, token checks answer 503"
    );
    let watch = host.clone();
    let seq = host.read().seq;
    let now_unix_ms: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(|| SystemClock.now_unix_ms());
    // The leaderboard (P3.5, D-051): its first build runs before `ready:`, so the board exists
    // from the first request; a failure there is loud, and the job retries every second. The job
    // runs on this runtime, so the shutdown drops it, and its host handle, with the connections.
    let (publish, board) = leaderboard::channel();
    let mut board_host = host.clone();
    match leaderboard::build_now(&mut board_host, now_unix_ms.as_ref(), epoch).await {
        Ok(built) => {
            publish.send_replace(Some(Arc::new(built)));
        }
        Err(error) => tracing::error!(
            %error,
            "the first leaderboard build failed; /leaderboard answers 503 until a build succeeds"
        ),
    }
    tokio::spawn(leaderboard::run(
        board_host,
        now_unix_ms.clone(),
        epoch,
        leaderboard_every,
        publish,
    ));
    let state = AppState {
        verifier,
        players: Arc::new(Players::new(accounts, host.clone())),
        host,
        now_unix_ms,
        epoch_unix_ms: epoch,
        leaderboard: board,
    };
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| fail(ServeExit::Input, &format!("bind {bind}: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| fail(ServeExit::Input, &format!("bind {bind}: {e}")))?;
    let (drain, drain_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, api::router(state))
            .with_graceful_shutdown(async {
                let _ = drain_rx.await;
            })
            .await
    });
    Ok(Started {
        task,
        watch,
        server,
        drain,
        addr,
        seq,
    })
}

/// Resolves once the host has stopped: its read model can't change any more.
async fn host_stopped(handle: &mut HostHandle) {
    while handle.changed().await.is_ok() {}
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
