//! A file-backed test log for the crash-and-retry harness (P2.8; decision log D-042): what was
//! durable survives the death of the process that wrote it, as with Postgres.
//!
//! - `records.jsonl`: each `append` writes its records' lines ([`LogRecord::to_line`]) and then
//!   one commit line, `{"commit":<last seq>}`, and calls `sync_all` before returning. Reading
//!   keeps only lines followed by a commit line, so an append torn by a death is all or none.
//! - `snapshots/<seq>.json`: each snapshot row, written to a temporary file and renamed.
//!
//! Every read loads the files into a `MemoryLog` and asks it, so the queries are the memory
//! store's.

use std::io::Write;
use std::path::{Path, PathBuf};

use voidforge_core::state::PlayerId;
use voidforge_store::log::{
    IdempotencyKey, LogError, LogRecord, LogStore, MemoryLog, SnapshotMeta, SnapshotRow,
};
use voidforge_store::verify::hex;

#[derive(Debug, Clone)]
pub struct FileLog {
    dir: PathBuf,
}

fn io(e: std::io::Error) -> LogError {
    LogError::Memory(format!("file log: {e}"))
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

impl FileLog {
    pub fn new(dir: &Path) -> Self {
        std::fs::create_dir_all(dir.join("snapshots")).unwrap();
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn records_path(&self) -> PathBuf {
        self.dir.join("records.jsonl")
    }

    /// The committed records: lines followed by a commit line.
    pub fn committed(&self) -> Result<Vec<LogRecord>, LogError> {
        let text = match std::fs::read_to_string(self.records_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(e)),
        };
        let (mut out, mut pending) = (Vec::new(), Vec::new());
        // A torn tail (no newline) is never followed by a commit line, so it is dropped with
        // the rest of its append.
        for line in text.split_inclusive('\n').filter(|l| l.ends_with('\n')) {
            let line = line.trim_end();
            if line.starts_with("{\"commit\":") {
                out.append(&mut pending);
            } else {
                pending.push(LogRecord::from_line(line)?);
            }
        }
        Ok(out)
    }

    pub fn snapshots(&self) -> Result<Vec<SnapshotRow>, LogError> {
        let mut rows = Vec::new();
        for entry in std::fs::read_dir(self.dir.join("snapshots")).map_err(io)? {
            let path = entry.map_err(io)?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).map_err(io)?)
                        .map_err(|e| LogError::Memory(e.to_string()))?;
                rows.push(SnapshotRow {
                    seq: v["seq"].as_i64().unwrap(),
                    game_time: v["game_time"].as_i64().unwrap(),
                    rules_version: u32::try_from(v["rules_version"].as_u64().unwrap()).unwrap(),
                    engine_build: v["engine_build"].as_str().unwrap().to_owned(),
                    state_hash: unhex(v["state_hash"].as_str().unwrap()),
                    payload: unhex(v["payload"].as_str().unwrap()),
                });
            }
        }
        rows.sort_by_key(|r| r.seq);
        Ok(rows)
    }

    async fn load(&self) -> Result<MemoryLog, LogError> {
        let mut m = MemoryLog::new();
        let records = self.committed()?;
        if !records.is_empty() {
            m.append(&records).await?;
        }
        for row in self.snapshots()? {
            m.write_snapshot(&row).await?;
        }
        Ok(m)
    }

    /// The lines an append writes: its records, then its commit line.
    pub fn append_bytes(records: &[LogRecord]) -> Result<Vec<u8>, LogError> {
        let mut text = String::new();
        for r in records {
            text.push_str(&r.to_line()?);
            text.push('\n');
        }
        let last = records.last().map_or(0, |r| r.seq);
        text.push_str(&format!("{{\"commit\":{last}}}\n"));
        Ok(text.into_bytes())
    }

    /// Discards whatever follows the last commit line (an append torn by a death), as a store
    /// does on its next write.
    pub fn discard_uncommitted(&self) -> Result<(), LogError> {
        let text = match std::fs::read(self.records_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io(e)),
        };
        let marker = b"{\"commit\":";
        let mut keep = 0;
        let mut at = 0;
        for line in text.split_inclusive(|&b| b == b'\n') {
            at += line.len();
            if line.starts_with(marker) && line.ends_with(b"\n") {
                keep = at;
            }
        }
        if keep < text.len() {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(self.records_path())
                .map_err(io)?;
            f.set_len(keep as u64).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        Ok(())
    }

    /// Writes `bytes` at the end of `records.jsonl` and syncs it.
    pub fn write_records(&self, bytes: &[u8], sync: bool) -> Result<(), LogError> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.records_path())
            .map_err(io)?;
        f.write_all(bytes).map_err(io)?;
        f.flush().map_err(io)?;
        if sync {
            f.sync_all().map_err(io)?;
        }
        Ok(())
    }

    pub fn snapshot_bytes(row: &SnapshotRow) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "seq": row.seq,
            "game_time": row.game_time,
            "rules_version": row.rules_version,
            "engine_build": row.engine_build,
            "state_hash": hex(&row.state_hash),
            "payload": hex(&row.payload),
        }))
        .unwrap()
    }

    /// Writes a snapshot row: a temporary file, then a rename.
    pub fn write_snapshot_bytes(
        &self,
        seq: i64,
        bytes: &[u8],
        rename: bool,
    ) -> Result<(), LogError> {
        let tmp = self.dir.join("snapshots").join(format!("{seq}.tmp"));
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        f.write_all(bytes).map_err(io)?;
        f.sync_all().map_err(io)?;
        if rename {
            std::fs::rename(&tmp, self.dir.join("snapshots").join(format!("{seq}.json")))
                .map_err(io)?;
        }
        Ok(())
    }
}

impl LogStore for FileLog {
    async fn last(&mut self) -> Result<Option<(i64, i64)>, LogError> {
        self.load().await?.last().await
    }

    async fn append(&mut self, records: &[LogRecord]) -> Result<(), LogError> {
        // The memory store's own checks (gap-free seq, unique keys) first.
        let mut check = self.load().await?;
        check.append(records).await?;
        self.discard_uncommitted()?;
        self.write_records(&Self::append_bytes(records)?, true)
    }

    async fn records(&mut self) -> Result<Vec<LogRecord>, LogError> {
        self.committed()
    }

    async fn find_keys(
        &mut self,
        keys: &[(PlayerId, IdempotencyKey)],
    ) -> Result<Vec<LogRecord>, LogError> {
        self.load().await?.find_keys(keys).await
    }

    async fn keyed_since(&mut self, since: i64) -> Result<Vec<LogRecord>, LogError> {
        self.load().await?.keyed_since(since).await
    }

    async fn records_after(&mut self, seq: i64) -> Result<Vec<LogRecord>, LogError> {
        self.load().await?.records_after(seq).await
    }

    async fn write_snapshot(&mut self, row: &SnapshotRow) -> Result<(), LogError> {
        self.write_snapshot_bytes(row.seq, &Self::snapshot_bytes(row), true)
    }

    async fn latest_snapshot(
        &mut self,
        build: Option<&str>,
    ) -> Result<Option<SnapshotRow>, LogError> {
        self.load().await?.latest_snapshot(build).await
    }

    async fn snapshot_index(&mut self) -> Result<Vec<SnapshotMeta>, LogError> {
        self.load().await?.snapshot_index().await
    }

    async fn snapshot_at(&mut self, seq: i64) -> Result<Option<SnapshotRow>, LogError> {
        self.load().await?.snapshot_at(seq).await
    }
}
