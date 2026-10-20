//! Server-rendered HTML frontend and OIDC login; reaches the server only over HTTP.
//!
//! Telemetry first (D-016), then the configuration and the issuer's discovery, each refusing
//! with a clear message before anything listens. Then it serves until SIGINT or SIGTERM, and
//! lets in-flight requests finish for at most [`DRAIN`]; a second signal stops it at once
//! (D-057). Sessions are in memory, so a restart logs everyone out; nothing else is lost.
//!
//! Exit codes: 0 after a clean stop; 1 when it can't start (configuration, discovery, the
//! address) or the drain timed out; 2 for a bad command line (clap's).

use std::future::IntoFuture;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use voidforge_web::config::{Args, SECRET_ENV};
use voidforge_web::oidc::Provider;
use voidforge_web::{api, routes};

const SERVICE: &str = "voidforge-web";
/// How long in-flight requests may finish after a stop signal.
const DRAIN: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    let telemetry = match voidforge_telemetry::init(SERVICE) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let code = run();
    match telemetry.shutdown() {
        Ok(()) => code,
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn fail(message: &str) -> ExitCode {
    tracing::error!("{message}");
    eprintln!("{SERVICE}: {message}");
    ExitCode::FAILURE
}

fn run() -> ExitCode {
    let args = Args::parse();
    let config = match args.into_config(std::env::var(SECRET_ENV).ok()) {
        Ok(c) => c,
        Err(e) => return fail(&format!("bad configuration: {e}")),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(&format!("the async runtime: {e}")),
    };
    let code = runtime.block_on(async move {
        let provider = match Provider::discover(&config).await {
            Ok(p) => p,
            Err(e) => return fail(&format!("the issuer {}: {e}", config.issuer)),
        };
        let api = match api::client() {
            Ok(c) => c,
            Err(e) => return fail(&format!("the API client: {e}")),
        };
        let (mut interrupt, mut terminate) = match (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) {
            (Ok(i), Ok(t)) => (i, t),
            (Err(e), _) | (_, Err(e)) => return fail(&format!("signal handlers: {e}")),
        };
        let listener = match tokio::net::TcpListener::bind(config.bind).await {
            Ok(l) => l,
            Err(e) => return fail(&format!("listening on {}: {e}", config.bind)),
        };
        let state = routes::AppState {
            provider: Arc::new(provider),
            api,
            api_url: config.api_url.clone(),
            sessions: voidforge_web::store::BoundedStore::default(),
        };
        let app = routes::app(state, config.secure_cookie);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .into_future();
        tokio::pin!(server);
        tracing::info!(bind = %config.bind, issuer = %config.issuer, "serving");
        println!("{SERVICE}: serving on http://{}", config.bind);
        let name = tokio::select! {
            result = &mut server => {
                return match result {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => fail(&format!("serving: {e}")),
                };
            }
            _ = interrupt.recv() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
        };
        tracing::info!(signal = name, drain_secs = DRAIN.as_secs(), "shutting down");
        let _ = stop_tx.send(());
        tokio::select! {
            result = tokio::time::timeout(DRAIN, &mut server) => match result {
                Ok(Ok(())) => ExitCode::SUCCESS,
                Ok(Err(e)) => fail(&format!("serving: {e}")),
                Err(_) => fail("in-flight requests didn't finish within the drain; stopping"),
            },
            _ = interrupt.recv() => fail("a second signal: stopping at once"),
            _ = terminate.recv() => fail("a second signal: stopping at once"),
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    code
}
