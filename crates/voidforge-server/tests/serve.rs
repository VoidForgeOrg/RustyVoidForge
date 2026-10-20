//! `voidforge-server serve` as a process (Technical Plan P3.4s; decision log D-050): the real
//! binary over a seeded `_sqlx_test_*` database and a dex stand-in, stopped with real signals.
//! The cases are the slice's proof contract: restart with the same epoch and a continuing game
//! time, recovery from the shutdown snapshot, the refusals, dex down, a request in flight at
//! shutdown, a second signal, SIGKILL, and the host dying while serving. These need Postgres, so
//! they are `#[ignore]`d outside the database stage (`just db-test`, the CI `database` job).
//!
//! Every wait is bounded: a child that doesn't print `ready:` or doesn't exit in time is killed,
//! and the test fails rather than hangs. The dex stand-in and every HTTP call run on threads of
//! their own, so a blocking wait on the child never starves them.

mod common;

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use common::dex::{Dex, TestKey, claims, discovery, jwks};
use serde_json::{Value, json};
use sqlx::PgPool;

const BIN: &str = env!("CARGO_BIN_EXE_voidforge-server");

/// The epoch the tests seed with: 2025-10-09T08:53:20Z, in the past, so game time runs.
const EPOCH: i64 = 1_760_000_000_000;
/// The window's ends (D-050), as the contract's numbers.
const EPOCH_END: i64 = 4_102_444_800_000;

/// How long a child may take to print `ready:` or to exit.
const PATIENCE: Duration = Duration::from_secs(30);

const ALICE_SUB: &str = "CgVhbGljZRIFbG9jYWw";
/// A command the core refuses (no such ship build), so it's a durable record without needing to
/// know any ID.
const COMMAND: &str = r#"{"type":"cancel_ship_build","payload":{"ship_build_id":1}}"#;

fn url(pool: &PgPool) -> String {
    let base = std::env::var("DATABASE_URL").expect("the database stage sets DATABASE_URL");
    let db = pool.connect_options().get_database().unwrap().to_owned();
    format!("{}/{db}", base.rsplit_once('/').unwrap().0)
}

fn rules_path() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../rules/v0.toml")
        .to_str()
        .unwrap()
        .to_owned()
}

/// `seed` with a small universe and `epoch`; its exit code.
fn seed(url: &str, epoch: i64) -> i32 {
    let out = Command::new(BIN)
        .args([
            "seed",
            "--seed",
            "7",
            "--players",
            "10",
            "--rules",
            &rules_path(),
        ])
        .args(["--epoch-unix-ms", &epoch.to_string(), "--json"])
        .env("DATABASE_URL", url)
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    out.status.code().unwrap()
}

// --- The dex stand-in, on a thread of its own ---

struct DexServer {
    dex: Arc<Dex>,
    key: TestKey,
    issuer: String,
}

impl DexServer {
    /// A stand-in on a free port.
    fn start() -> Self {
        Self::on(std::net::TcpListener::bind("127.0.0.1:0").unwrap())
    }

    /// A stand-in on `listener`.
    fn on(listener: std::net::TcpListener) -> Self {
        let addr = listener.local_addr().unwrap();
        let issuer = format!("http://{addr}/dex");
        let dex = Arc::new(Dex::default());
        let key = TestKey::new("k1");
        *dex.keys.lock().unwrap() = vec![key.jwk.clone()];
        *dex.issuer.lock().unwrap() = issuer.clone();
        let state = dex.clone();
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let router = axum::Router::new()
                    .route(
                        "/dex/.well-known/openid-configuration",
                        axum::routing::get(discovery),
                    )
                    .route("/dex/keys", axum::routing::get(jwks))
                    .with_state(state);
                axum::serve(listener, router).await.unwrap();
            });
        });
        Self { dex, key, issuer }
    }

    fn token(&self, sub: &str) -> String {
        let mut c = claims(&self.issuer);
        c["sub"] = json!(sub);
        self.key.sign(&c)
    }
}

// --- HTTP, on a thread of its own per call ---

#[derive(Debug)]
struct Reply {
    status: u16,
    retry_after: Option<String>,
    body: Value,
}

/// A request started on its own thread: `Err` when the connection failed.
fn request(
    method: &'static str,
    url: String,
    token: Option<String>,
    key: Option<&'static str>,
) -> JoinHandle<Result<Reply, String>> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let client = reqwest::Client::new();
            let mut r = match method {
                "POST" => client
                    .post(&url)
                    .header("content-type", "application/json")
                    .body(COMMAND),
                _ => client.get(&url),
            };
            if let Some(t) = token {
                r = r.bearer_auth(t);
            }
            if let Some(k) = key {
                r = r.header("idempotency-key", k);
            }
            let response = r
                .timeout(PATIENCE)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .map(|v| v.to_str().unwrap().to_owned());
            let text = response.text().await.map_err(|e| e.to_string())?;
            Ok(Reply {
                status,
                retry_after,
                body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
            })
        })
    })
}

fn call(
    method: &'static str,
    url: String,
    token: Option<String>,
    key: Option<&'static str>,
) -> Reply {
    request(method, url, token, key).join().unwrap().unwrap()
}

// --- The process ---

struct Serve {
    child: Child,
    addr: String,
    ready: String,
    stderr: Arc<Mutex<String>>,
}

/// Starts `serve` and waits (bounded) for its `ready:` line.
fn serve(url: &str, issuer: &str, extra: &[&str]) -> Serve {
    let mut child = Command::new(BIN)
        .args(["serve", "--database-url", url, "--rules", &rules_path()])
        .args([
            "--bind",
            "127.0.0.1:0",
            "--issuer",
            issuer,
            "--audience",
            "voidforge-cli",
        ])
        .args(extra)
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("DATABASE_URL")
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = Arc::new(Mutex::new(String::new()));
    let sink = stderr.clone();
    let mut err = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok(n) = err.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let (lines, rx) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            if lines.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let ready = loop {
        match rx.recv_timeout(PATIENCE) {
            Ok(line) if line.starts_with("ready: ") => break line,
            Ok(_) => {}
            Err(_) => {
                let _ = child.kill();
                let status = child.wait().unwrap();
                panic!(
                    "no ready line ({status}); stderr:\n{}",
                    stderr.lock().unwrap()
                );
            }
        }
    };
    let addr = ready
        .strip_prefix("ready: listening on ")
        .and_then(|r| r.split(',').next())
        .unwrap()
        .to_owned();
    Serve {
        child,
        addr,
        ready,
        stderr,
    }
}

impl Serve {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn signal(&self, name: &str) {
        let ok = Command::new("kill")
            .args(["-s", name, &self.child.id().to_string()])
            .status()
            .unwrap()
            .success();
        assert!(ok, "kill -s {name}");
    }

    /// The `ready:` line's seq.
    fn seq(&self) -> i64 {
        self.ready.rsplit_once("seq ").unwrap().1.parse().unwrap()
    }

    /// The `ready:` line's epoch text.
    fn epoch(&self) -> String {
        let after = self.ready.split_once("epoch ").unwrap().1;
        after.split(',').next().unwrap().to_owned()
    }

    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    /// Waits until stderr contains `text`, bounded.
    fn logged(&self, text: &str) {
        let deadline = Instant::now() + PATIENCE;
        while !self.stderr().contains(text) {
            assert!(
                Instant::now() < deadline,
                "never logged {text:?}:\n{}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the exit, bounded by `within`; its code and stderr. A child still running then
    /// is killed and the test fails.
    fn exit(mut self, within: Duration) -> (i32, String) {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // The stderr reader ends with the pipe; give it a moment to drain.
                std::thread::sleep(Duration::from_millis(100));
                let code = status.code().unwrap_or(-1);
                return (code, self.stderr());
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "serve didn't exit within {within:?}; stderr:\n{}",
                    self.stderr()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

/// A failing test never leaves a `serve` running: it would hold its test database's sessions,
/// and the next run's cleanup couldn't drop it.
impl Drop for Serve {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// --- What Postgres holds ---

/// (the newest snapshot's seq, the log head).
async fn snapshot_and_head(pool: &PgPool) -> (i64, i64) {
    let snapshot: i64 = sqlx::query_scalar("SELECT max(seq) FROM snapshots")
        .fetch_one(pool)
        .await
        .unwrap();
    let head: i64 = sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM log_records")
        .fetch_one(pool)
        .await
        .unwrap();
    (snapshot, head)
}

async fn epoch_row(pool: &PgPool) -> Option<i64> {
    sqlx::query_scalar("SELECT epoch_unix_ms FROM universe")
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn tables(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Waits until the host's append is waiting on a lock: the request is certainly in flight.
async fn append_waits_on_a_lock(pool: &PgPool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND wait_event_type = 'Lock'
                AND query LIKE 'INSERT INTO log_records%'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the append never waited on the lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A seeded universe, a dex stand-in, and alice joined through a first call.
async fn seeded(pool: &PgPool) -> (String, DexServer) {
    let url = url(pool);
    assert_eq!(seed(&url, EPOCH), 0);
    (url, DexServer::start())
}

// --- The cases ---

/// The done-when: seed, serve, act, SIGINT → exit 0 with the final snapshot at the head; serve
/// again → the same epoch, recovered from that snapshot (nothing left to replay), and game time
/// continues.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn sigint_stops_cleanly_and_the_next_start_recovers_the_same_universe(pool: PgPool) {
    restart_cycle(&pool, "INT").await;
}

/// The same with SIGTERM, which compose sends.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn sigterm_stops_cleanly_and_the_next_start_recovers_the_same_universe(pool: PgPool) {
    restart_cycle(&pool, "TERM").await;
}

async fn restart_cycle(pool: &PgPool, signal: &str) {
    let (url, dex) = seeded(pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.seq(), 0);
    assert_eq!(s.epoch(), "2025-10-09T08:53:20.000Z");
    // The first call joins alice; then a command, refused by the core but durable.
    let me = call("GET", s.url("/players/me"), Some(token.clone()), None);
    assert_eq!(me.status, 200, "{:?}", me.body);
    let first = call("POST", s.url("/commands"), Some(token.clone()), Some("k-1"));
    assert_eq!(first.status, 422, "{:?}", first.body);
    s.signal(signal);
    let (code, stderr) = s.exit(PATIENCE);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains(&format!("signal=\"SIG{signal}\"")),
        "{stderr}"
    );
    let (snapshot, head) = snapshot_and_head(pool).await;
    assert!(head >= 2, "the join and the command are records");
    assert_eq!(snapshot, head, "the final snapshot is at the log head");
    assert_eq!(epoch_row(pool).await, Some(EPOCH), "the epoch is unchanged");
    let last_time: i64 = sqlx::query_scalar("SELECT max(game_time) FROM log_records")
        .fetch_one(pool)
        .await
        .unwrap();

    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.epoch(), "2025-10-09T08:53:20.000Z", "the same epoch");
    assert_eq!(s.seq(), head, "recovered to the head");
    let recovered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM log_records WHERE seq > (SELECT max(seq) FROM snapshots)",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        recovered, 0,
        "recovered from the shutdown snapshot: no tail to replay"
    );
    // A repeated key gets its first answer; a new command's game time is later.
    let again = call("POST", s.url("/commands"), Some(token.clone()), Some("k-1"));
    assert_eq!(
        again.body, first.body,
        "the key's first answer, across the restart"
    );
    let later = call("POST", s.url("/commands"), Some(token), Some("k-2"));
    assert_eq!(later.status, 422);
    let times: Vec<i64> = sqlx::query_scalar("SELECT game_time FROM log_records ORDER BY seq")
        .fetch_all(pool)
        .await
        .unwrap();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    let later_time: i64 =
        sqlx::query_scalar("SELECT game_time FROM log_records WHERE idempotency_key = 'k-2'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        later_time > last_time,
        "game time continues: {later_time} after {last_time}"
    );
    s.signal("TERM");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// A database without a universe is refused before anything is migrated: `serve` never creates
/// a schema (the lead's T2 amendment).
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn an_empty_database_is_refused_and_left_without_tables(pool: PgPool) {
    let dex = DexServer::start();
    assert_eq!(tables(&pool).await, 0);
    let out = Command::new(BIN)
        .args([
            "serve",
            "--database-url",
            &url(&pool),
            "--rules",
            &rules_path(),
        ])
        .args(["--bind", "127.0.0.1:0", "--issuer", &dex.issuer])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("no universe; seed it first"), "{stderr}");
    assert!(!stderr.contains("password"), "the URL is never printed");
    assert_eq!(tables(&pool).await, 0, "nothing was migrated");
}

/// A universe seeded before the epoch was recorded (reproduced by effect: the row removed) is
/// refused with the next step, and nothing is written. An epoch is never invented.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_universe_without_a_recorded_epoch_is_refused(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    sqlx::query("DELETE FROM universe")
        .execute(&pool)
        .await
        .unwrap();
    let before = snapshot_and_head(&pool).await;
    let out = Command::new(BIN)
        .args(["serve", "--database-url", &url, "--rules", &rules_path()])
        .args(["--bind", "127.0.0.1:0", "--issuer", &dex.issuer])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("seeded before its epoch was recorded"),
        "{stderr}"
    );
    assert!(stderr.contains("just seed-reset"), "{stderr}");
    assert_eq!(snapshot_and_head(&pool).await, before);
    assert_eq!(epoch_row(&pool).await, None, "no epoch was invented");
}

/// `seed` refuses an epoch outside [1970, 2100) before connecting (the database stays empty),
/// and accepts the window's last millisecond.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn seed_refuses_an_epoch_outside_the_window(pool: PgPool) {
    let url = url(&pool);
    for epoch in [-1, EPOCH_END, i64::MAX, i64::MIN] {
        assert_eq!(seed(&url, epoch), 3, "epoch {epoch}");
        assert_eq!(tables(&pool).await, 0, "nothing written for {epoch}");
    }
    assert_eq!(seed(&url, EPOCH_END - 1), 0);
    assert_eq!(epoch_row(&pool).await, Some(EPOCH_END - 1));
}

/// `serve` refuses a recorded epoch outside the window, apart from the table's CHECK (dropped
/// here to write one), and the CHECK itself holds the window, by effect.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn serve_refuses_a_recorded_epoch_outside_the_window_and_the_check_holds_it(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    for (epoch, ok) in [
        (-1, false),
        (EPOCH_END, false),
        (0, true),
        (EPOCH_END - 1, true),
    ] {
        let r = sqlx::query("UPDATE universe SET epoch_unix_ms = $1")
            .bind(epoch)
            .execute(&pool)
            .await;
        match r {
            Ok(_) => assert!(ok, "the CHECK let {epoch} through"),
            Err(sqlx::Error::Database(e)) => {
                assert!(!ok, "{epoch}: {e}");
                assert_eq!(e.code().as_deref(), Some("23514"));
                assert_eq!(e.constraint(), Some("universe_epoch_in_window"));
            }
            Err(e) => panic!("{e}"),
        }
    }
    let second = sqlx::query("INSERT INTO universe (epoch_unix_ms) VALUES ($1)")
        .bind(EPOCH)
        .execute(&pool)
        .await
        .unwrap_err();
    let second = second.as_database_error().unwrap();
    assert_eq!(second.code().as_deref(), Some("23505"), "one row only");
    sqlx::query("ALTER TABLE universe DROP CONSTRAINT universe_epoch_in_window")
        .execute(&pool)
        .await
        .unwrap();
    for epoch in [-1, EPOCH_END] {
        sqlx::query("UPDATE universe SET epoch_unix_ms = $1")
            .bind(epoch)
            .execute(&pool)
            .await
            .unwrap();
        let out = Command::new(BIN)
            .args(["serve", "--database-url", &url, "--rules", &rules_path()])
            .args(["--bind", "127.0.0.1:0", "--issuer", &dex.issuer])
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{epoch}: {stderr}");
        assert!(
            stderr.contains("outside 1970-01-01 to 2100-01-01"),
            "{stderr}"
        );
    }
}

/// The window's last millisecond serves end to end: the wall clock is far behind the epoch, so
/// game time is held at 0 (stamps never go negative), each record's `wall_time` is the real wall
/// clock, and the views' `as_of` is the epoch itself, formatted. This is the record path
/// the P3.4 QA's `wall_time` carry was about: the epoch never reaches that column.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn an_epoch_at_the_windows_end_serves_and_its_records_are_durable(pool: PgPool) {
    let url = url(&pool);
    assert_eq!(seed(&url, EPOCH_END - 1), 0);
    let dex = DexServer::start();
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.epoch(), "2099-12-31T23:59:59.999Z");
    let token = dex.token(ALICE_SUB);
    let me = call("GET", s.url("/players/me"), Some(token.clone()), None);
    assert_eq!(me.status, 200, "{:?}", me.body);
    assert_eq!(me.body["as_of"], "2099-12-31T23:59:59.999Z");
    let r = call("POST", s.url("/commands"), Some(token), Some("edge"));
    assert_eq!(r.status, 422, "{:?}", r.body);
    let times: Vec<i64> = sqlx::query_scalar("SELECT game_time FROM log_records")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        !times.is_empty() && times.iter().all(|&t| t == 0),
        "{times:?}"
    );
    let wall_ok: bool = sqlx::query_scalar(
        "SELECT bool_and(wall_time BETWEEN now() - interval '1 hour' AND now()) FROM log_records",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(wall_ok, "wall_time is the real wall clock");
    s.signal("INT");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// dex down at startup (D-046): `serve` starts anyway; a token gets 503 `unavailable` with
/// Retry-After, never an accept, and `/openapi.json` answers; once dex comes up, the keys load
/// within the backoff and the token is accepted.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn dex_down_at_startup_answers_503_until_the_keys_load(pool: PgPool) {
    let url = url(&pool);
    assert_eq!(seed(&url, EPOCH), 0);
    // A port with nothing listening: taken, then freed.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let issuer = format!("http://127.0.0.1:{port}/dex");
    let s = serve(&url, &issuer, &[]);
    assert_eq!(call("GET", s.url("/openapi.json"), None, None).status, 200);
    // Signed by the key the stand-in will publish, for the issuer serve was given.
    let key = TestKey::new("k1");
    let mut c = claims(&issuer);
    c["sub"] = json!(ALICE_SUB);
    let token = key.sign(&c);
    let r = call("GET", s.url("/players/me"), Some(token.clone()), None);
    assert_eq!(r.status, 503, "{:?}", r.body);
    assert_eq!(r.body["code"], "unavailable");
    assert!(r.retry_after.is_some());
    // dex comes up on that port, publishing the key.
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let dex = DexServer::on(listener);
    *dex.dex.keys.lock().unwrap() = vec![key.jwk.clone()];
    let deadline = Instant::now() + PATIENCE;
    loop {
        let r = call("GET", s.url("/players/me"), Some(token.clone()), None);
        if r.status == 200 {
            break;
        }
        assert_eq!(
            r.status, 503,
            "never anything but 503 before the keys load: {:?}",
            r.body
        );
        assert!(Instant::now() < deadline, "the keys never loaded");
        std::thread::sleep(Duration::from_millis(250));
    }
    s.signal("TERM");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// A request in flight at shutdown, made certain with no hook in the code: the test holds
/// `log_records` locked, so the host's append waits, and the drain waits for it. Released within
/// the drain, the request is answered, its record is durable, and the stop is clean.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_request_in_flight_at_shutdown_finishes_within_the_drain(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(
        call("GET", s.url("/players/me"), Some(token.clone()), None).status,
        200
    );
    let (_, head) = snapshot_and_head(&pool).await;
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE log_records IN EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let in_flight = request("POST", s.url("/commands"), Some(token), Some("flight"));
    append_waits_on_a_lock(&pool).await;
    s.signal("TERM");
    s.logged("shutting down");
    lock.commit().await.unwrap();
    let reply = in_flight
        .join()
        .unwrap()
        .expect("answered within the drain");
    assert_eq!(reply.status, 422, "{:?}", reply.body);
    let (code, stderr) = s.exit(PATIENCE);
    assert_eq!(code, 0, "{stderr}");
    let (snapshot, now_head) = snapshot_and_head(&pool).await;
    assert_eq!(now_head, head + 1, "the in-flight command is durable");
    assert_eq!(snapshot, now_head);
}

/// Held past the drain: the connection is cut off and the stop is unclean (exit 6). Once the
/// cut-off request's handle is gone, the host still stops and writes its snapshot, and a restart
/// answers a retry of the same key with the first receipt.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_request_held_past_the_drain_is_cut_off_and_the_stop_is_unclean(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &["--drain-secs", "1"]);
    assert_eq!(
        call("GET", s.url("/players/me"), Some(token.clone()), None).status,
        200
    );
    let (_, head) = snapshot_and_head(&pool).await;
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE log_records IN EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let in_flight = request(
        "POST",
        s.url("/commands"),
        Some(token.clone()),
        Some("held"),
    );
    append_waits_on_a_lock(&pool).await;
    s.signal("TERM");
    s.logged("cutting them off");
    // The host is still waiting in its append; let it finish now.
    lock.commit().await.unwrap();
    let (code, stderr) = s.exit(PATIENCE);
    assert_eq!(code, 6, "{stderr}");
    assert!(
        in_flight.join().unwrap().is_err(),
        "the client saw its connection cut"
    );
    let (snapshot, now_head) = snapshot_and_head(&pool).await;
    assert_eq!(now_head, head + 1, "the append the host was in completed");
    assert_eq!(
        snapshot, now_head,
        "the host stopped and wrote its snapshot anyway"
    );
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.seq(), now_head);
    let retry = call("POST", s.url("/commands"), Some(token), Some("held"));
    assert_eq!(retry.status, 422);
    assert_eq!(
        snapshot_and_head(&pool).await.1,
        now_head,
        "the retry appended nothing"
    );
    s.signal("INT");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// A second signal while draining exits at once with 6; recovery then holds.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_second_signal_exits_at_once_and_recovery_holds(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &["--drain-secs", "60"]);
    assert_eq!(
        call("GET", s.url("/players/me"), Some(token.clone()), None).status,
        200
    );
    let (_, head) = snapshot_and_head(&pool).await;
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE log_records IN EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let in_flight = request(
        "POST",
        s.url("/commands"),
        Some(token.clone()),
        Some("second"),
    );
    append_waits_on_a_lock(&pool).await;
    s.signal("INT");
    s.logged("shutting down");
    let started = Instant::now();
    s.signal("INT");
    let (code, stderr) = s.exit(Duration::from_secs(5));
    assert_eq!(code, 6, "{stderr}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "at once, not after the 60 s drain"
    );
    assert!(stderr.contains("second signal"), "{stderr}");
    lock.rollback().await.unwrap();
    let _ = in_flight.join().unwrap();
    // The process is gone, so its append's transaction is too: the command is absent.
    let (snapshot, now_head) = snapshot_and_head(&pool).await;
    assert_eq!(
        now_head, head,
        "the unacknowledged command was never written"
    );
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.seq(), now_head);
    assert!(snapshot <= now_head);
    let retry = call("POST", s.url("/commands"), Some(token), Some("second"));
    assert_eq!(retry.status, 422);
    assert_eq!(
        snapshot_and_head(&pool).await.1,
        now_head + 1,
        "applied once, now"
    );
    s.signal("INT");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// SIGKILL while serving: no shutdown snapshot, and the next start replays the log from the
/// seq-0 snapshot, with every acknowledged command in it.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn after_sigkill_serve_recovers_from_the_log(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &[]);
    let me = call("GET", s.url("/players/me"), Some(token.clone()), None);
    assert_eq!(me.status, 200);
    let first = call(
        "POST",
        s.url("/commands"),
        Some(token.clone()),
        Some("kill-1"),
    );
    assert_eq!(first.status, 422);
    s.kill();
    let (snapshot, head) = snapshot_and_head(&pool).await;
    assert_eq!(snapshot, 0, "no shutdown snapshot");
    assert!(head >= 2);
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.seq(), head, "the tail was replayed");
    let after = call("GET", s.url("/players/me"), Some(token.clone()), None).body;
    // The same player and homeworld (as_of is the request's now, so it differs).
    assert_eq!(after["player_id"], me.body["player_id"]);
    assert_eq!(after["homeworld_id"], me.body["homeworld_id"]);
    let again = call("POST", s.url("/commands"), Some(token), Some("kill-1"));
    assert_eq!(
        again.body, first.body,
        "the acknowledged command is in the log"
    );
    s.signal("TERM");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// The host dies while serving: an append fails (a raising trigger, which a superuser can't
/// bypass, unlike a revoked privilege), the host stops, and `serve` exits 6 rather than answer
/// 503 forever. The failure is proven to have fired: the host's fatal error is logged. Without
/// the trigger, a restart recovers.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn the_host_dying_while_serving_exits_6_and_a_restart_recovers(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(
        call("GET", s.url("/players/me"), Some(token.clone()), None).status,
        200
    );
    let (_, head) = snapshot_and_head(&pool).await;
    sqlx::query(
        "CREATE FUNCTION refuse_append() RETURNS trigger LANGUAGE plpgsql AS
           $$ BEGIN RAISE EXCEPTION 'the test refuses this append'; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER refuse_append BEFORE INSERT ON log_records
           FOR EACH ROW EXECUTE FUNCTION refuse_append()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let r = call(
        "POST",
        s.url("/commands"),
        Some(token.clone()),
        Some("dies"),
    );
    assert_eq!(r.status, 503, "{:?}", r.body);
    let (code, stderr) = s.exit(PATIENCE);
    assert_eq!(code, 6, "{stderr}");
    assert!(
        stderr.contains("the test refuses this append"),
        "the failure fired:\n{stderr}"
    );
    assert!(stderr.contains("the host stopped on its own"), "{stderr}");
    assert_eq!(
        snapshot_and_head(&pool).await.1,
        head,
        "nothing was written"
    );
    sqlx::query("DROP TRIGGER refuse_append ON log_records")
        .execute(&pool)
        .await
        .unwrap();
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(s.seq(), head);
    assert_eq!(
        call("POST", s.url("/commands"), Some(token), Some("dies")).status,
        422
    );
    s.signal("INT");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// A database migrated but never seeded holds no universe either: exit 3, "seed it first".
#[sqlx::test(migrator = "voidforge_store::MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn a_migrated_but_unseeded_database_is_refused(pool: PgPool) {
    let dex = DexServer::start();
    let out = Command::new(BIN)
        .args([
            "serve",
            "--database-url",
            &url(&pool),
            "--rules",
            &rules_path(),
        ])
        .args(["--bind", "127.0.0.1:0", "--issuer", &dex.issuer])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("no universe; seed it first"), "{stderr}");
}

/// An epoch row without a universe (only possible by hand) counts as used: `seed` refuses rather
/// than half-seeds, and writes nothing.
#[sqlx::test(migrator = "voidforge_store::MIGRATOR")]
#[ignore = "needs Postgres: just db-test"]
async fn seed_refuses_a_database_holding_only_an_epoch(pool: PgPool) {
    sqlx::query("INSERT INTO universe (epoch_unix_ms) VALUES ($1)")
        .bind(EPOCH)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(seed(&url(&pool), EPOCH + 1), 3);
    assert_eq!(epoch_row(&pool).await, Some(EPOCH));
    let snapshots: i64 = sqlx::query_scalar("SELECT count(*) FROM snapshots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(snapshots, 0);
}

/// Another rules version than the universe's is a refusal (3), not an input error: Rules v0-fast
/// is version 1, and this universe was seeded under version 0.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn another_rules_version_is_refused(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let fast = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules/v0-fast.toml");
    let out = Command::new(BIN)
        .args([
            "serve",
            "--database-url",
            &url,
            "--rules",
            fast.to_str().unwrap(),
        ])
        .args(["--bind", "127.0.0.1:0", "--issuer", &dex.issuer])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("rules version"), "{stderr}");
}

/// A final snapshot that fails to write (a raising trigger on `snapshots`) isn't an error to the
/// host's join, only a counter; `serve` reads Postgres, finds no snapshot at the head, and exits 6.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_final_snapshot_that_fails_is_an_unclean_stop(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(
        call("GET", s.url("/players/me"), Some(token), None).status,
        200
    );
    sqlx::query(
        "CREATE FUNCTION refuse_snapshot() RETURNS trigger LANGUAGE plpgsql AS
           $$ BEGIN RAISE EXCEPTION 'the test refuses this snapshot'; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER refuse_snapshot BEFORE INSERT ON snapshots
           FOR EACH ROW EXECUTE FUNCTION refuse_snapshot()",
    )
    .execute(&pool)
    .await
    .unwrap();
    s.signal("INT");
    let (code, stderr) = s.exit(PATIENCE);
    assert_eq!(code, 6, "{stderr}");
    assert!(
        stderr.contains("the test refuses this snapshot"),
        "the failure fired:\n{stderr}"
    );
    assert!(stderr.contains("is not at the log head"), "{stderr}");
    let (snapshot, head) = snapshot_and_head(&pool).await;
    assert!(snapshot < head);
}

/// The store refuses an epoch outside the window itself, before connecting: the URL here leads
/// nowhere, and the answer is still the epoch.
#[tokio::test]
async fn bootstrap_refuses_an_epoch_outside_the_window_before_connecting() {
    let rules = voidforge_rules::parse(&std::fs::read_to_string(rules_path()).unwrap()).unwrap();
    let world = voidforge_worldgen::generate(7, 1, &rules).unwrap();
    let build = voidforge_store::snapshot::EngineBuild::new("serve-test").unwrap();
    let snap = voidforge_store::snapshot::encode(&world, &build).unwrap();
    for epoch in [-1, EPOCH_END] {
        let e = voidforge_store::bootstrap("postgres://u@127.0.0.1:1/none", &snap, epoch)
            .await
            .unwrap_err();
        assert!(
            matches!(e, voidforge_store::BootstrapError::EpochOutOfRange(x) if x == epoch),
            "{e}"
        );
    }
}

// --- Repair round 1: every shutdown step watches for a second signal and is bounded (review-r2's
// M1, QA's D1), the join bound (Q2), seed's default epoch (Q3), and one serve per universe (I4) ---

/// Makes the host's shutdown snapshot take at least 2 s: an AFTER INSERT trigger that sleeps.
async fn slow_snapshots(pool: &PgPool) {
    sqlx::query(
        "CREATE FUNCTION slow_snapshot() RETURNS trigger LANGUAGE plpgsql AS
           $$ BEGIN PERFORM pg_sleep(2); RETURN NEW; END $$",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER slow_snapshot AFTER INSERT ON snapshots
           FOR EACH ROW EXECUTE FUNCTION slow_snapshot()",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Waits (bounded) until a session in this database runs a statement starting with `query` and
/// waits on `wait`: a `wait_event_type` ('Lock') or a `wait_event` ('PgSleep').
async fn waits(pool: &PgPool, query: &str, wait: &str) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND (wait_event_type = $2 OR wait_event = $2)
                AND query LIKE $1 || '%'",
        )
        .bind(query)
        .bind(wait)
        .fetch_one(pool)
        .await
        .unwrap();
        if n > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{query:?} never waited on {wait}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Holds the final verification's SELECT behind a snapshots lock (review-r2's probe): the
/// shutdown snapshot's INSERT is slowed, and an ACCESS EXCLUSIVE lock queued behind it is granted
/// after it commits and before the check reads. Returns the transaction holding the lock.
async fn hold_the_final_check<'a>(
    pool: &'a PgPool,
    s: &Serve,
) -> sqlx::Transaction<'a, sqlx::Postgres> {
    slow_snapshots(pool).await;
    let mut tx = pool.begin().await.unwrap();
    s.signal("INT");
    waits(pool, "INSERT INTO snapshots", "PgSleep").await;
    sqlx::query("LOCK TABLE snapshots IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .unwrap();
    waits(
        pool,
        "SELECT seq, game_time, rules_version, engine_build, state_hash, payload",
        "Lock",
    )
    .await;
    tx
}

/// Review-r2's M1, as a regression: a second signal while the final check waits on Postgres
/// exits 6 at once, rather than waiting for the query and then exiting 0.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_second_signal_during_the_final_check_exits_6_at_once(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(
        call(
            "GET",
            s.url("/players/me"),
            Some(dex.token(ALICE_SUB)),
            None
        )
        .status,
        200
    );
    let tx = hold_the_final_check(&pool, &s).await;
    s.signal("TERM");
    let (code, stderr) = s.exit(Duration::from_secs(2));
    assert_eq!(code, 6, "{stderr}");
    assert!(stderr.contains("while checking the snapshot"), "{stderr}");
    tx.rollback().await.unwrap();
}

/// The final check is bounded: held past 5 s with no second signal, it's exit 6, not a wait for
/// as long as Postgres holds it.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn the_final_check_is_bounded_at_5_s(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let s = serve(&url, &dex.issuer, &[]);
    assert_eq!(
        call(
            "GET",
            s.url("/players/me"),
            Some(dex.token(ALICE_SUB)),
            None
        )
        .status,
        200
    );
    let tx = hold_the_final_check(&pool, &s).await;
    let held = Instant::now();
    let (code, stderr) = s.exit(Duration::from_secs(10));
    let after = held.elapsed();
    assert_eq!(code, 6, "{stderr}");
    assert!(stderr.contains("did not finish within 5 s"), "{stderr}");
    assert!(
        after > Duration::from_secs(4) && after < Duration::from_secs(8),
        "{after:?}"
    );
    tx.rollback().await.unwrap();
}

/// QA's D1, as a regression: one signal stream covers every step from the first signal to the
/// exit, so a second signal at any moment of the shutdown is exit 6. Each round appends a record
/// first, so the stop always has a snapshot to write, and that write is slowed to 2 s, so every
/// delay here falls inside the stop. (Without a new record the newest snapshot is already at the
/// head, nothing is written, and a stop takes under 100 ms.) The exit must come within 1 s of the
/// second signal, inside the 2 s the snapshot holds the stop: a step that ignored the signal would
/// leave it pending for a later step, and exit 6 only after the snapshot. The first signal is SIGINT
/// and the second SIGTERM, which the kernel can't merge into one delivery.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_second_signal_at_any_moment_of_the_shutdown_exits_6(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    slow_snapshots(&pool).await;
    let token = dex.token(ALICE_SUB);
    for delay_ms in [0, 2, 5, 10, 20, 40, 80, 200, 1000] {
        let s = serve(&url, &dex.issuer, &[]);
        let key: &'static str = Box::leak(format!("sweep-{delay_ms}").into_boxed_str());
        let r = call("POST", s.url("/commands"), Some(token.clone()), Some(key));
        assert_eq!(r.status, 422, "{:?}", r.body);
        let (snapshot, head) = snapshot_and_head(&pool).await;
        assert!(snapshot < head, "the stop has a snapshot to write");
        s.signal("INT");
        std::thread::sleep(Duration::from_millis(delay_ms));
        s.signal("TERM");
        // At once: well inside the 2 s the slowed snapshot holds the stop, so a step that
        // ignores the signal (and leaves it pending for a later step) fails here.
        let (code, stderr) = s.exit(Duration::from_secs(1));
        assert_eq!(
            code, 6,
            "second signal {delay_ms} ms after the first:\n{stderr}"
        );
        assert!(
            stderr.contains("second signal (SIGTERM)"),
            "{delay_ms} ms: {stderr}"
        );
    }
}

/// QA's Q2: the join is bounded at 10 s. The host can't stop while its append waits on a held
/// lock, so one signal gives exit 6 about 10 s later (no drain, a 1 s margin either side).
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn the_host_join_is_bounded_at_10_s(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &["--drain-secs", "0"]);
    assert_eq!(
        call("GET", s.url("/players/me"), Some(token.clone()), None).status,
        200
    );
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE log_records IN EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let in_flight = request("POST", s.url("/commands"), Some(token), Some("bound"));
    append_waits_on_a_lock(&pool).await;
    s.signal("INT");
    let sent = Instant::now();
    let (code, stderr) = s.exit(Duration::from_secs(20));
    let after = sent.elapsed();
    assert_eq!(code, 6, "{stderr}");
    assert!(stderr.contains("did not stop within 10 s"), "{stderr}");
    assert!(
        after > Duration::from_secs(9) && after < Duration::from_secs(13),
        "{after:?}"
    );
    lock.rollback().await.unwrap();
    let _ = in_flight.join().unwrap();
}

/// QA's Q3: seed's default epoch is the wall clock when it runs.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn seed_defaults_the_epoch_to_the_wall_clock(pool: PgPool) {
    let now = || {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    };
    let before = now();
    let out = Command::new(BIN)
        .args([
            "seed",
            "--seed",
            "7",
            "--players",
            "10",
            "--rules",
            &rules_path(),
        ])
        .env("DATABASE_URL", url(&pool))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let after = now();
    assert_eq!(out.status.code(), Some(0));
    let epoch = epoch_row(&pool).await.unwrap();
    assert!(
        (before..=after).contains(&epoch),
        "{before} <= {epoch} <= {after}"
    );
}

/// QA's I4, the lead's addition: one serve per universe. A second gets exit 3 before it recovers
/// or binds, and the first serves on, unaffected, and stops cleanly. Once it has stopped, the
/// universe can be served again.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn a_second_serve_on_the_same_universe_is_refused(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let first = serve(&url, &dex.issuer, &[]);
    let out = Command::new(BIN)
        .args(["serve", "--database-url", &url, "--rules", &rules_path()])
        .args(["--bind", "127.0.0.1:0", "--issuer", &dex.issuer])
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("another serve already holds this universe"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("recovered the universe"),
        "refused before recovery"
    );
    assert!(!stdout.contains("ready:"), "refused before binding");
    assert_eq!(
        call("GET", first.url("/players/me"), Some(token), None).status,
        200
    );
    first.signal("INT");
    assert_eq!(first.exit(PATIENCE).0, 0);
    let again = serve(&url, &dex.issuer, &[]);
    again.signal("INT");
    assert_eq!(again.exit(PATIENCE).0, 0);
}

// --- The leaderboard job inside serve (P3.5, D-051) ---

/// The board exists from the first request (the first build runs before `ready:`), and with
/// `--leaderboard-secs 1` its `as_of` advances within a few seconds.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn the_board_is_there_at_ready_and_its_as_of_advances(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    let s = serve(&url, &dex.issuer, &["--leaderboard-secs", "1"]);
    let first = call("GET", s.url("/leaderboard"), Some(token.clone()), None);
    assert_eq!(first.status, 200, "{:?}", first.body);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let now = call("GET", s.url("/leaderboard"), Some(token.clone()), None);
        assert_eq!(now.status, 200);
        if now.body["as_of"].as_str() > first.body["as_of"].as_str() {
            assert!(
                now.body["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["rank"] == 1),
                "alice joined, so she's on it: {:?}",
                now.body
            );
            break;
        }
        assert!(Instant::now() < deadline, "the as_of never advanced");
        std::thread::sleep(Duration::from_millis(200));
    }
    s.signal("INT");
    assert_eq!(s.exit(PATIENCE).0, 0);
}

/// The lead's T3 condition, explicit: the job holds a host handle while it waits (here, asleep for
/// an hour between builds), and the host's join still completes within its bound, because the
/// shutdown drops the job's task with the server runtime. A job holding its handle elsewhere would
/// keep the host alive: exit 6 after the 10 s join bound.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn the_host_joins_with_the_job_mid_wait(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let s = serve(&url, &dex.issuer, &["--leaderboard-secs", "3600"]);
    assert_eq!(
        call(
            "GET",
            s.url("/players/me"),
            Some(dex.token(ALICE_SUB)),
            None
        )
        .status,
        200
    );
    s.signal("INT");
    let sent = Instant::now();
    let (code, stderr) = s.exit(PATIENCE);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains("the final snapshot is at the log head"),
        "{stderr}"
    );
    assert!(
        sent.elapsed() < Duration::from_secs(5),
        "joined well inside the 10 s bound: {:?}",
        sent.elapsed()
    );
}

/// Dev-qa's Q1: the job's interval is pinned. With `--leaderboard-secs 1`, every gap between new
/// `as_of`s over 6 s lies between 0.8 and 1.6 s: a job building ten times as often (or not at
/// all) fails here.
#[sqlx::test(migrations = false)]
#[ignore = "needs Postgres: just db-test"]
async fn the_job_rebuilds_on_its_interval_and_no_faster(pool: PgPool) {
    let (url, dex) = seeded(&pool).await;
    let token = dex.token(ALICE_SUB);
    // It polls at 20 a second, over the default read rate (D-054): this test is about the
    // board's interval, so its reads get room.
    let s = serve(
        &url,
        &dex.issuer,
        &[
            "--leaderboard-secs",
            "1",
            "--read-rate",
            "1000",
            "--read-burst",
            "10000",
        ],
    );
    let mut last =
        call("GET", s.url("/leaderboard"), Some(token.clone()), None).body["as_of"].clone();
    let mut seen_at = Vec::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(6) {
        let now = call("GET", s.url("/leaderboard"), Some(token.clone()), None);
        assert_eq!(now.status, 200);
        if now.body["as_of"] != last {
            last = now.body["as_of"].clone();
            seen_at.push(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        seen_at.len() >= 3,
        "at least three rebuilds in 6 s: {}",
        seen_at.len()
    );
    for w in seen_at.windows(2) {
        let gap = w[1] - w[0];
        assert!(
            gap > Duration::from_millis(800) && gap < Duration::from_millis(1_600),
            "a rebuild gap of {gap:?}"
        );
    }
    s.signal("INT");
    assert_eq!(s.exit(PATIENCE).0, 0);
}
