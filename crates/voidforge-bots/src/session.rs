//! The saved terminal login (D-053): the ID token from `voidforge-bots login`, kept for later
//! commands in one user-only file. A token is a credential, so it's never printed or logged, the
//! file is created 0600 (in a 0700 directory) and replaced atomically, and a file anyone else can
//! read is refused, as ssh refuses a readable key. There's no refresh token: when the login
//! expires (dex's 24 h ID-token lifetime), the person logs in again.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Failure;

/// The variable naming the token file, overriding the default location.
pub const FILE_VAR: &str = "VOIDFORGE_TOKEN_FILE";

/// What "log in again" says, with the command.
pub const LOG_IN_AGAIN: &str = "log in again: voidforge-bots login";

/// A saved login.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// The dex issuer the token came from.
    pub issuer: String,
    /// The ID token. A credential: never print or log it.
    pub id_token: String,
    /// When it expires, in Unix seconds.
    pub expires_at: i64,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("issuer", &self.issuer)
            .field("id_token", &"<hidden>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Where the login is saved: `$VOIDFORGE_TOKEN_FILE`, else `$XDG_CONFIG_HOME/voidforge/token.json`,
/// else `~/.config/voidforge/token.json`.
///
/// # Errors
/// [`Failure::Usage`] when neither variable nor `HOME` names a directory.
pub fn path() -> Result<PathBuf, Failure> {
    let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(file) = set(FILE_VAR) {
        return Ok(PathBuf::from(file));
    }
    let config = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| set("HOME").map(|home| Path::new(&home).join(".config")))
        .ok_or_else(|| {
            Failure::Usage(format!(
                "no place to save the login: set {FILE_VAR}, XDG_CONFIG_HOME or HOME"
            ))
        })?;
    Ok(config.join("voidforge").join("token.json"))
}

/// Now, in Unix seconds.
#[must_use]
pub fn now_unix_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Saves `session` at `file`: its directory created 0700 if missing, the file written 0600 from
/// the start to a temporary name beside it, then renamed over the old one.
///
/// # Errors
/// [`Failure::Usage`] when the file can't be written.
pub fn save(file: &Path, session: &Session) -> Result<(), Failure> {
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        Failure::Usage(format!(
            "can't save the login at {}: {what}: {e}",
            file.display()
        ))
    };
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| fail("its directory", &e))?;
    let bytes = serde_json::to_vec(session).map_err(|e| fail("encoding", &e))?;
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        file.file_name()
            .map_or_else(|| "token".into(), |n| n.to_string_lossy()),
        std::process::id()
    ));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut f| f.write_all(&bytes).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&temp, file));
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(fail("writing", &e));
    }
    Ok(())
}

/// The saved login at `file`, if it's still usable.
///
/// # Errors
/// [`Failure::LogInAgain`] when there's none, when anyone but its owner can read or write it (with
/// the `chmod` that fixes it), when it can't be read, or when it has expired.
pub fn load(file: &Path) -> Result<Session, Failure> {
    let meta = match fs::metadata(file) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Failure::LogInAgain(format!(
                "you're not logged in; {LOG_IN_AGAIN}"
            )));
        }
        Err(e) => {
            return Err(Failure::LogInAgain(format!(
                "can't read the saved login at {}: {e}; {LOG_IN_AGAIN}",
                file.display()
            )));
        }
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(Failure::LogInAgain(format!(
            "the saved login at {} can be read by others (mode {mode:03o}), so it isn't used; \
             fix it with `chmod 600 {}`, or delete it and log in again",
            file.display(),
            file.display()
        )));
    }
    let text = fs::read_to_string(file).map_err(|e| {
        Failure::LogInAgain(format!(
            "can't read the saved login at {}: {e}; {LOG_IN_AGAIN}",
            file.display()
        ))
    })?;
    let session: Session = serde_json::from_str(&text).map_err(|_| {
        Failure::LogInAgain(format!(
            "the saved login at {} isn't one this tool wrote; {LOG_IN_AGAIN}",
            file.display()
        ))
    })?;
    if session.expires_at <= now_unix_s() {
        return Err(Failure::LogInAgain(format!(
            "your login has expired; {LOG_IN_AGAIN}"
        )));
    }
    Ok(session)
}

/// Deletes the saved login; whether there was one.
///
/// # Errors
/// [`Failure::Usage`] when it exists but can't be deleted.
pub fn delete(file: &Path) -> Result<bool, Failure> {
    match fs::remove_file(file) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Failure::Usage(format!(
            "can't delete the saved login at {}: {e}",
            file.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vf-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn session(expires_at: i64) -> Session {
        Session {
            issuer: "http://localhost:5556/dex".to_owned(),
            id_token: "header.payload.signature".to_owned(),
            expires_at,
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_saved_login_is_user_only_in_a_user_only_directory() {
        let dir = temp_dir("modes");
        let file = dir.join("voidforge").join("token.json");
        save(&file, &session(now_unix_s() + 60)).unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        // Replacing it keeps the mode and leaves no temporary file behind.
        save(&file, &session(now_unix_s() + 120)).unwrap();
        assert_eq!(mode(&file), 0o600);
        let names: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["token.json"]);
        assert!(load(&file).unwrap().expires_at > now_unix_s() + 60);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_login_others_can_read_is_refused_with_the_fix() {
        let dir = temp_dir("readable");
        let file = dir.join("token.json");
        save(&file, &session(now_unix_s() + 60)).unwrap();
        for bad in [0o640, 0o604, 0o620, 0o602] {
            fs::set_permissions(&file, fs::Permissions::from_mode(bad)).unwrap();
            let Err(Failure::LogInAgain(message)) = load(&file) else {
                panic!("mode {bad:o} accepted");
            };
            assert!(message.contains("chmod 600"), "{message}");
            assert!(!message.contains("header.payload"), "{message}");
        }
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&file).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_expired_or_missing_login_says_log_in_again() {
        let dir = temp_dir("expired");
        let file = dir.join("token.json");
        let Err(Failure::LogInAgain(missing)) = load(&file) else {
            panic!("a missing login loaded");
        };
        assert!(missing.contains(LOG_IN_AGAIN), "{missing}");
        save(&file, &session(now_unix_s() - 1)).unwrap();
        let Err(Failure::LogInAgain(expired)) = load(&file) else {
            panic!("an expired login loaded");
        };
        assert_eq!(expired, format!("your login has expired; {LOG_IN_AGAIN}"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn delete_removes_the_login_and_says_whether_there_was_one() {
        let dir = temp_dir("delete");
        let file = dir.join("token.json");
        save(&file, &session(now_unix_s() + 60)).unwrap();
        assert!(delete(&file).unwrap());
        assert!(!file.exists());
        assert!(!delete(&file).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_debug_form_hides_the_token() {
        let shown = format!("{:?}", session(1));
        assert!(!shown.contains("header.payload"), "{shown}");
    }
}
