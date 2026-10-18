//! The replay and divergence finder (Technical Plan P2.7; decision log D-041).
//!
//! It rebuilds a universe offline from a snapshot and the log, through the same record→core
//! function as host recovery (`voidforge_store::replay`, via `voidforge_store::verify` and
//! `voidforge_store::golden`), verifies every record's events and outcome and every state hash
//! the source records, and reports the first divergent record. It never touches live state.
//!
//! Exit codes (a stable contract, D-041): 0 verified; 1 diverged; 2 usage error; 3 refused (the
//! source is pinned to another build or rules version); 4 input error; 5 telemetry failed. With
//! several sources each is reported, and the exit code is the first failing source's.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use voidforge_core::rules::Rules;
use voidforge_store::golden::{self, FILES, Files};
use voidforge_store::log::{LogRecord, LogStore, PgLog};
use voidforge_store::snapshot::{self, ENGINE_BUILD, EngineBuild};
use voidforge_store::verify::{self, Finding, Report, VerifyError};

const SERVICE: &str = "voidforge-replay";

const BUILD_HELP: &str = "The build the log was written by, asserted (default: this binary's own \
    build). A pass under --build means this binary's code reproduces that log, not that the \
    named build's code was verified: a production incident log is verified by its own build's \
    binary.";

const EXIT_CODES: &str = "Exit codes: 0 verified, 1 diverged, 2 usage error, 3 refused (another \
    build or rules version), 4 input error, 5 telemetry failed.";

/// Replays a Voidforge log offline and reports the first divergent record.
#[derive(Debug, Parser)]
#[command(name = SERVICE, version = ENGINE_BUILD, about, long_about = None, after_help = EXIT_CODES)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Verifies golden logs: each golden directory, or each golden in a directory of them.
    Golden {
        /// Golden directories, or directories of goldens (for example tests/golden).
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// The rules file (default: rules/v<N>.toml in the nearest ancestor that has it).
        #[arg(long)]
        rules: Option<PathBuf>,
        /// One JSON document on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Verifies a Postgres log from a snapshot to the head.
    Postgres(PostgresArgs),
    /// Reports what a source holds, without verifying anything.
    #[command(subcommand)]
    Inspect(Inspect),
}

#[derive(Debug, Args)]
struct PostgresArgs {
    /// The database (default: DATABASE_URL). It is never printed.
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    /// The rules file; its version must be the log's.
    #[arg(long)]
    rules: PathBuf,
    /// The snapshot to start from, by seq (default: the earliest).
    #[arg(long)]
    from: Option<i64>,
    /// The last record to replay (default: the head).
    #[arg(long)]
    to: Option<i64>,
    #[arg(long, help = BUILD_HELP)]
    build: Option<String>,
    /// One JSON document on stdout.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum Inspect {
    /// Reports goldens' metadata, unverified.
    Golden {
        /// Golden directories, or directories of goldens.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// One JSON document on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Reports a Postgres log's snapshots and records, unverified.
    Postgres {
        /// The database (default: DATABASE_URL). It is never printed.
        #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
        database_url: String,
        /// One JSON document on stdout.
        #[arg(long)]
        json: bool,
    },
}

/// The exit codes (D-041). 2 is clap's, for usage errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    Verified = 0,
    Diverged = 1,
    Refused = 3,
    Input = 4,
    Telemetry = 5,
}

impl Exit {
    fn of(e: &VerifyError) -> Self {
        match e {
            VerifyError::Diverged(_) => Self::Diverged,
            VerifyError::Refused(_) => Self::Refused,
            VerifyError::Input(_) => Self::Input,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Diverged => "diverged",
            Self::Refused => "refused",
            Self::Input => "error",
            Self::Telemetry => "telemetry",
        }
    }
}

/// One source's result, as printed.
#[derive(Debug, Serialize)]
struct Source {
    source: String,
    result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<String>,
    build_from_flag: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rules_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    records: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoints: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    divergence: Option<FindingOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip)]
    exit: Exit,
}

#[derive(Debug, Serialize)]
struct FindingOut {
    seq: Option<i64>,
    what: &'static str,
    expected: String,
    found: String,
}

impl Source {
    fn new(source: String, build: Option<String>, build_from_flag: bool) -> Self {
        Self {
            source,
            result: Exit::Verified.word(),
            build,
            build_from_flag,
            rules_version: None,
            records: None,
            first_seq: None,
            last_seq: None,
            checkpoints: None,
            end_hash: None,
            divergence: None,
            error: None,
            exit: Exit::Verified,
        }
    }

    fn with(mut self, outcome: Result<Report, VerifyError>) -> Self {
        match outcome {
            Ok(r) => {
                self.build = Some(r.build);
                self.rules_version = Some(r.rules_version);
                self.records = Some(r.records);
                self.first_seq = r.first_seq;
                self.last_seq = Some(r.last_seq);
                self.checkpoints = Some(r.checkpoints);
                self.end_hash = Some(r.end_hash);
            }
            Err(e) => {
                self.exit = Exit::of(&e);
                self.result = self.exit.word();
                match e {
                    VerifyError::Diverged(f) | VerifyError::Refused(f) => {
                        let Finding {
                            seq,
                            what,
                            expected,
                            found,
                        } = f;
                        self.divergence = Some(FindingOut {
                            seq,
                            what,
                            expected,
                            found,
                        });
                    }
                    VerifyError::Input(message) => self.error = Some(message),
                }
            }
        }
        self
    }

    fn text(&self) -> String {
        let mut out = match (self.exit, &self.divergence, &self.error) {
            (Exit::Verified, _, _) => format!(
                "verified {}: build {}, rules v{}, {} records{}, {} hashes checked, end {}",
                self.source,
                self.build.as_deref().unwrap_or("?"),
                self.rules_version.unwrap_or_default(),
                self.records.unwrap_or_default(),
                seq_range(self.first_seq, self.last_seq),
                self.checkpoints.unwrap_or_default(),
                self.end_hash.as_deref().unwrap_or("?"),
            ),
            (_, Some(d), _) => format!(
                "{} {}: {} at {}\n  expected: {}\n  found:    {}",
                self.result.to_uppercase(),
                self.source,
                d.what,
                d.seq
                    .map_or_else(|| "the start or end".to_owned(), |s| format!("seq {s}")),
                d.expected,
                d.found
            ),
            (_, None, Some(e)) => format!("ERROR {}: {e}", self.source),
            (_, None, None) => format!("{} {}", self.result.to_uppercase(), self.source),
        };
        if self.build_from_flag {
            out.push_str(&format!(
                "\n  build {} asserted by --build: a pass means this binary reproduces the log, \
                 not that build's code",
                self.build.as_deref().unwrap_or("?")
            ));
        }
        out
    }
}

fn seq_range(first: Option<i64>, last: Option<i64>) -> String {
    match (first, last) {
        (Some(a), Some(b)) => format!(" (seq {a}..{b})"),
        _ => String::new(),
    }
}

#[derive(Debug, Serialize)]
struct Output<T: Serialize> {
    result: &'static str,
    sources: Vec<T>,
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
    let span = tracing::info_span!("replay", binary = SERVICE);
    let exit = span.in_scope(|| run(cli));
    if let Some(trace_id) = voidforge_telemetry::trace_id(&span) {
        eprintln!("trace_id={trace_id}");
    }
    drop(span);
    match telemetry.shutdown() {
        Ok(()) => ExitCode::from(exit as u8),
        Err(error) => {
            eprintln!("{SERVICE}: {error}");
            // A failed shutdown is never a success (D-016); a failing verification keeps its code.
            let code = if exit == Exit::Verified {
                Exit::Telemetry
            } else {
                exit
            };
            ExitCode::from(code as u8)
        }
    }
}

fn run(cli: Cli) -> Exit {
    match cli.command {
        Command::Golden { paths, rules, json } => {
            let sources = goldens(&paths)
                .into_iter()
                .map(|found| match found {
                    Ok(dir) => verify_golden_dir(&dir, rules.as_deref()),
                    Err((path, e)) => Source::new(path, None, false).with(Err(e)),
                })
                .collect();
            print_sources(sources, json)
        }
        Command::Postgres(args) => {
            let (build, from_flag) = match &args.build {
                Some(b) => (b.clone(), true),
                None => (ENGINE_BUILD.to_owned(), false),
            };
            let outcome = postgres(&args, &build);
            let source = Source::new("postgres".to_owned(), Some(build), from_flag).with(outcome);
            print_sources(vec![source], args.json)
        }
        Command::Inspect(Inspect::Golden { paths, json }) => {
            let reports: Vec<Inventory> = goldens(&paths)
                .into_iter()
                .map(|found| match found {
                    Ok(dir) => inspect_golden(&dir),
                    Err((path, e)) => Inventory::failed(path, &e),
                })
                .collect();
            print_inventories(reports, json)
        }
        Command::Inspect(Inspect::Postgres { database_url, json }) => {
            let report = inspect_postgres(&database_url)
                .unwrap_or_else(|e| Inventory::failed("postgres".to_owned(), &e));
            print_inventories(vec![report], json)
        }
    }
}

fn print_sources(sources: Vec<Source>, json: bool) -> Exit {
    let exit = sources
        .iter()
        .map(|s| s.exit)
        .find(|&e| e != Exit::Verified)
        .unwrap_or(Exit::Verified);
    if json {
        let out = Output {
            result: exit.word(),
            sources,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&out).expect("output serializes")
        );
    } else {
        for s in &sources {
            println!("{}", s.text());
        }
    }
    exit
}

// --- Goldens ---

type Found = Result<PathBuf, (String, VerifyError)>;

/// The golden directories under `paths`: a path with a manifest is one golden; otherwise its
/// subdirectories with manifests, sorted. A path with none is an input error, and so is anything
/// that can't be read on the way: an unreadable entry is never skipped (phase 2 gate sweep). A
/// directory with a golden's other files but no manifest is an incomplete golden, an input error
/// naming it, never left out of a sweep (phase 2 gate, round 2).
fn goldens(paths: &[PathBuf]) -> Vec<Found> {
    let mut out = Vec::new();
    for path in paths {
        let failed = |at: &Path, e: String| Err((at.display().to_string(), VerifyError::Input(e)));
        match kind(path) {
            Ok(Kind::Golden) => {
                out.push(Ok(path.clone()));
                continue;
            }
            Ok(Kind::Incomplete) => {
                out.push(failed(path, INCOMPLETE.to_owned()));
                continue;
            }
            Ok(Kind::Other) => {}
            Err(e) => {
                out.push(failed(path, e));
                continue;
            }
        }
        let children = std::fs::read_dir(path)
            .map_err(|e| e.to_string())
            .and_then(|entries| {
                let mut children = Vec::new();
                for entry in entries {
                    let dir = entry.map_err(|e| e.to_string())?.path();
                    match kind(&dir) {
                        Ok(Kind::Other) => {}
                        Ok(k) => children.push((dir, Ok(k))),
                        Err(e) => children.push((dir, Err(e))),
                    }
                }
                Ok(children)
            });
        match children {
            Err(e) => out.push(failed(path, e)),
            Ok(children) if children.is_empty() => {
                out.push(failed(path, "no golden here (no manifest.json)".to_owned()));
            }
            Ok(mut children) => {
                children.sort_by(|a, b| a.0.cmp(&b.0));
                out.extend(children.into_iter().map(|(dir, k)| match k {
                    Ok(Kind::Golden) => Ok(dir),
                    Ok(_) => failed(&dir, INCOMPLETE.to_owned()),
                    Err(e) => failed(&dir, e),
                }));
            }
        }
    }
    out
}

const INCOMPLETE: &str = "an incomplete golden: it has golden files but no manifest.json";

/// What a directory is to the golden search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// It holds a manifest.
    Golden,
    /// It holds a golden's snapshot or records, but no manifest.
    Incomplete,
    /// Anything else, which a search passes over (a README, an unrelated directory).
    Other,
}

/// What `dir` is: a missing file is absent; any other failure to look is an error.
fn kind(dir: &Path) -> Result<Kind, String> {
    if has_file(dir, FILES[2])? {
        Ok(Kind::Golden)
    } else if has_file(dir, FILES[0])? || has_file(dir, FILES[1])? {
        Ok(Kind::Incomplete)
    } else {
        Ok(Kind::Other)
    }
}

fn has_file(dir: &Path, name: &str) -> Result<bool, String> {
    let file = dir.join(name);
    match std::fs::metadata(&file) {
        Ok(m) => Ok(m.is_file()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Ok(false),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

fn read_golden(dir: &Path) -> Result<Files, VerifyError> {
    let read = |name: &str| {
        std::fs::read(dir.join(name)).map_err(|e| VerifyError::Input(format!("{name}: {e}")))
    };
    Ok(Files {
        start: read(FILES[0])?,
        records: read(FILES[1])?,
        manifest: read(FILES[2])?,
    })
}

fn verify_golden_dir(dir: &Path, rules: Option<&Path>) -> Source {
    let source = Source::new(
        dir.display().to_string(),
        Some(golden::FIXTURE.to_owned()),
        false,
    );
    let outcome = read_golden(dir).and_then(|files| {
        let version = files.manifest()?.rules_version;
        let rules = load_rules(rules, dir, version)?;
        golden::verify_golden(&files, &rules)
    });
    source.with(outcome)
}

/// The rules: the file given, or `rules/v<version>.toml` in the nearest ancestor of `near`.
fn load_rules(given: Option<&Path>, near: &Path, version: u32) -> Result<Rules, VerifyError> {
    let path = match given {
        Some(p) => p.to_path_buf(),
        None => {
            let name = format!("rules/v{version}.toml");
            std::path::absolute(near)
                .map_err(|e| VerifyError::Input(e.to_string()))?
                .ancestors()
                .map(|a| a.join(&name))
                .find(|p| p.is_file())
                .ok_or_else(|| {
                    VerifyError::Input(format!("no {name} found above the source; pass --rules"))
                })?
        }
    };
    read_rules(&path)
}

fn read_rules(path: &Path) -> Result<Rules, VerifyError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| VerifyError::Input(format!("{}: {e}", path.display())))?;
    voidforge_rules::parse(&text)
        .map_err(|e| VerifyError::Input(format!("{}: {e}", path.display())))
}

// --- Postgres ---

fn runtime() -> Result<tokio::runtime::Runtime, VerifyError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| VerifyError::Input(format!("runtime: {e}")))
}

fn connect(url: &str) -> Result<PgLog, VerifyError> {
    // The URL holds a password: never echo it.
    PgLog::lazy_url(url)
        .map_err(|_| VerifyError::Input("--database-url is not a Postgres URL".to_owned()))
}

fn postgres(args: &PostgresArgs, build: &str) -> Result<Report, VerifyError> {
    let build = EngineBuild::new(build).map_err(|e| VerifyError::Input(format!("--build: {e}")))?;
    let rules = read_rules(&args.rules)?;
    runtime()?.block_on(async {
        let mut store = connect(&args.database_url)?;
        verify::verify_log(&mut store, &rules, &build, args.from, args.to).await
    })
}

// --- Inspect: unverified metadata (D-035's caveat on decode_any) ---

#[derive(Debug, Serialize)]
struct Inventory {
    source: String,
    /// Always false: nothing here was verified.
    verified: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    snapshots: Vec<SnapshotInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    records: Option<RecordsInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SnapshotInfo {
    seq: i64,
    engine_build: String,
    rules_version: u32,
    game_time_ms: i64,
    state_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<u16>,
}

#[derive(Debug, Serialize)]
struct RecordsInfo {
    count: usize,
    first_seq: Option<i64>,
    last_seq: Option<i64>,
    engine_builds: BTreeSet<String>,
    rules_versions: BTreeSet<u32>,
}

impl RecordsInfo {
    fn of(records: &[LogRecord]) -> Self {
        Self {
            count: records.len(),
            first_seq: records.first().map(|r| r.seq),
            last_seq: records.last().map(|r| r.seq),
            engine_builds: records
                .iter()
                .map(|r| r.engine_build.as_str().to_owned())
                .collect(),
            rules_versions: records.iter().map(|r| r.rules_version).collect(),
        }
    }
}

impl Inventory {
    fn failed(source: String, e: &VerifyError) -> Self {
        Self {
            source,
            verified: false,
            snapshots: Vec::new(),
            records: None,
            error: Some(e.to_string()),
        }
    }

    fn text(&self) -> String {
        let mut out = format!("UNVERIFIED {}", self.source);
        for s in &self.snapshots {
            out.push_str(&format!(
                "\n  snapshot seq {}: build {}, rules v{}, game time {} ms, hash {}{}",
                s.seq,
                s.engine_build,
                s.rules_version,
                s.game_time_ms,
                s.state_hash,
                s.format
                    .map_or_else(String::new, |f| format!(", format {f}")),
            ));
        }
        if let Some(r) = &self.records {
            out.push_str(&format!(
                "\n  records: {}{}, builds {:?}, rules versions {:?}",
                r.count,
                seq_range(r.first_seq, r.last_seq),
                r.engine_builds,
                r.rules_versions,
            ));
        }
        if let Some(e) = &self.error {
            out.push_str(&format!("\n  error: {e}"));
        }
        out
    }
}

fn inspect_golden(dir: &Path) -> Inventory {
    let source = dir.display().to_string();
    let inventory = || -> Result<Inventory, VerifyError> {
        let files = read_golden(dir)?;
        // decode_any: build metadata unverified, for inspection only (D-035).
        let header = snapshot::decode_any(&files.start)
            .map_err(|e| VerifyError::Input(format!("start.vfsnap: {e}")))?
            .header;
        let records = files.records()?;
        Ok(Inventory {
            source: source.clone(),
            verified: false,
            snapshots: vec![SnapshotInfo {
                seq: 0,
                engine_build: header.engine_build,
                rules_version: header.rules_version,
                game_time_ms: header.game_time_ms,
                state_hash: verify::hex(&header.state_hash),
                format: Some(header.format),
            }],
            records: Some(RecordsInfo::of(&records)),
            error: None,
        })
    };
    inventory().unwrap_or_else(|e| Inventory::failed(source.clone(), &e))
}

fn inspect_postgres(url: &str) -> Result<Inventory, VerifyError> {
    runtime()?.block_on(async {
        let mut store = connect(url)?;
        let snapshots = store
            .snapshot_index()
            .await?
            .into_iter()
            .map(|s| SnapshotInfo {
                seq: s.seq,
                engine_build: s.engine_build,
                rules_version: s.rules_version,
                game_time_ms: s.game_time,
                state_hash: verify::hex(&s.state_hash),
                format: None,
            })
            .collect();
        let records = store.records().await?;
        Ok(Inventory {
            source: "postgres".to_owned(),
            verified: false,
            snapshots,
            records: Some(RecordsInfo::of(&records)),
            error: None,
        })
    })
}

fn print_inventories(reports: Vec<Inventory>, json: bool) -> Exit {
    let exit = if reports.iter().any(|r| r.error.is_some()) {
        Exit::Input
    } else {
        Exit::Verified
    };
    if json {
        let out = Output {
            result: if exit == Exit::Input {
                "error"
            } else {
                "unverified"
            },
            sources: reports,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&out).expect("output serializes")
        );
    } else {
        for r in &reports {
            println!("{}", r.text());
        }
    }
    exit
}
