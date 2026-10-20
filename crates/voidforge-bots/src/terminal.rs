//! Playing from a terminal (Technical Plan P3.7; decision log D-053): `login` through dex's device
//! code flow, `logout`, and `me`, `get` and `command` against the API with the saved login.
//!
//! Answers go to stdout as JSON, so they can be piped; everything for the person goes to stderr.
//! The token, the subject and the device code are never printed: the person sees the
//! verification URL, the user code, and the idempotency key a command used.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use voidforge_api_types::command::CommandRequest;
use voidforge_api_types::error::ErrorCode;

use crate::Failure;
use crate::client::{Player, Refused, Reply};
use crate::login::{self, Prompt};
use crate::session::{self, LOG_IN_AGAIN, Session};
use crate::wait::utc;

/// The name the client's errors use for the person.
const YOU: &str = "you";

/// `login`: the device flow at `issuer`, then the login saved at `file`.
///
/// # Errors
/// What [`login::device`] and [`session::save`] return.
pub async fn login(http: &reqwest::Client, issuer: &str, file: &Path) -> Result<(), Failure> {
    let token = login::device(http, issuer, show).await?;
    let lifetime = i64::try_from(token.expires_in.as_secs()).unwrap_or(i64::MAX);
    let expires_at = session::now_unix_s().saturating_add(lifetime);
    session::save(
        file,
        &Session {
            issuer: issuer.to_owned(),
            id_token: token.token,
            expires_at,
        },
    )?;
    eprintln!(
        "Logged in. Your login is valid until {} and is saved in {}.",
        utc(expires_at),
        file.display()
    );
    Ok(())
}

/// What the person must do, on stderr: the one link to open, the plain URL and the code, and that
/// the tool is waiting until the code expires.
fn show(prompt: &Prompt) {
    match &prompt.complete {
        Some(link) => eprintln!("To log in, open this link in a browser:\n\n    {link}\n"),
        None => eprintln!("To log in, open the link below in a browser.\n"),
    }
    eprintln!(
        "Or open {} and enter the code {}.",
        prompt.uri, prompt.user_code
    );
    eprintln!(
        "Waiting for you to approve the login; the code expires in {} minutes.",
        prompt.expires_in.as_secs().div_ceil(60)
    );
}

/// `logout`: deletes the saved login.
///
/// # Errors
/// What [`session::delete`] returns.
pub fn logout(file: &Path) -> Result<(), Failure> {
    if session::delete(file)? {
        eprintln!("Logged out: deleted {}.", file.display());
    } else {
        eprintln!("You weren't logged in ({} doesn't exist).", file.display());
    }
    Ok(())
}

/// `me`: `GET /players/me` with the saved login.
///
/// # Errors
/// [`Failure::LogInAgain`] without a usable login; [`Failure::Refused`] when the API refuses.
pub async fn me(http: &reqwest::Client, api: &str, file: &Path) -> Result<(), Failure> {
    let (player, saved) = player(http, api, file)?;
    print_json(answer(player.get_json("/players/me").await?, &saved)?)
}

/// `get`: any `GET` the API offers, by its path. Only a path on the configured API: never a URL,
/// so this is not a general HTTP client.
///
/// # Errors
/// [`Failure::Usage`] for something that isn't an API path; otherwise as [`me`].
pub async fn get(
    http: &reqwest::Client,
    api: &str,
    file: &Path,
    path: &str,
) -> Result<(), Failure> {
    check_path(path)?;
    let (player, saved) = player(http, api, file)?;
    print_json(answer(player.get_json(path).await?, &saved)?)
}

/// `command`: `POST /commands` with `json`, a `CommandRequest` checked locally first, under `key`
/// or a fresh one; the key is printed so the person can repeat the command deliberately.
///
/// # Errors
/// [`Failure::Usage`] for JSON that isn't a command or a key the API wouldn't take; otherwise as
/// [`me`].
pub async fn command(
    http: &reqwest::Client,
    api: &str,
    file: &Path,
    json: &str,
    key: Option<String>,
) -> Result<(), Failure> {
    let request: CommandRequest =
        serde_json::from_str(json).map_err(|e| Failure::Usage(format!("not a command: {e}")))?;
    let key = match key {
        Some(key) => voidforge_api_types::command::IdempotencyKey::parse(Some(&key))
            .map_err(|_| {
                Failure::Usage(
                    "--key must be 1 to 128 bytes of printable ASCII without spaces".to_owned(),
                )
            })?
            .as_str()
            .to_owned(),
        None => fresh_key(),
    };
    let (player, saved) = player(http, api, file)?;
    eprintln!("Idempotency key: {key} (repeat with --key {key} to retry this exact command)");
    print_json(answer(player.command(&request, &key).await?, &saved)?)
}

/// A client with the saved login, and the login's issuer and expiry for the messages.
fn player(http: &reqwest::Client, api: &str, file: &Path) -> Result<(Player, Saved), Failure> {
    let session = session::load(file)?;
    let saved = Saved {
        issuer: session.issuer,
        expires_at: session.expires_at,
    };
    Ok((Player::new(YOU, http.clone(), api, session.id_token), saved))
}

/// What a refusal's message needs from the saved login: never the token.
struct Saved {
    issuer: String,
    expires_at: i64,
}

/// An API answer: a 401 means the login no longer works, and says why as far as the tool can tell
/// (dev-qa's F2): past its saved expiry it has expired; before it, the API refused a login that
/// should still be valid, most likely one from another dex, so the message names where it came
/// from. Any other refusal is the API's.
fn answer<T>(reply: Reply<T>, saved: &Saved) -> Result<T, Failure> {
    reply.map_err(|Refused { status, body }| {
        if status == 401 && matches!(body.code, ErrorCode::InvalidToken | ErrorCode::MissingToken) {
            if saved.expires_at <= session::now_unix_s() {
                Failure::LogInAgain(format!("your login has expired; {LOG_IN_AGAIN}"))
            } else {
                Failure::LogInAgain(format!(
                    "the API refused your saved login (it may be from another dex: you logged in at \
                     {}); {LOG_IN_AGAIN}",
                    saved.issuer
                ))
            }
        } else {
            Failure::Refused(format!(
                "the API refused (HTTP {status}): {}: {}",
                serde_json::to_value(body.code).map_or_else(|_| "?".to_owned(), |c| c.to_string()),
                body.message
            ))
        }
    })
}

/// An API path: it starts with one `/`, and has no scheme, host or whitespace.
fn check_path(path: &str) -> Result<(), Failure> {
    let ok = path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("://")
        && !path.contains('\\')
        && path.bytes().all(|b| b.is_ascii_graphic());
    if ok {
        Ok(())
    } else {
        Err(Failure::Usage(format!(
            "{path:?} isn't an API path; give one like /players/me, /planets/27 or /leaderboard"
        )))
    }
}

/// A fresh idempotency key: the time in nanoseconds, the process ID and a counter, so it's unique
/// for one person even when the clock repeats a reading.
fn fresh_key() -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("cli-{nanos}-{}-{n}", std::process::id())
}

fn print_json(value: impl serde::Serialize) -> Result<(), Failure> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| Failure::Http(format!("can't print the answer: {e}")))?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_takes_only_api_paths() {
        for ok in [
            "/players/me",
            "/planets/27",
            "/planets?near=1,2,3&radius=10",
            "/leaderboard",
        ] {
            assert!(check_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "players/me",
            "//evil.example/x",
            "http://evil.example/",
            "/x://y",
            "/a b",
            "/a\\b",
            "",
        ] {
            assert!(matches!(check_path(bad), Err(Failure::Usage(_))), "{bad:?}");
        }
    }

    #[test]
    fn fresh_keys_are_valid_and_differ() {
        let (a, b) = (fresh_key(), fresh_key());
        assert_ne!(a, b);
        for key in [a, b] {
            assert!(voidforge_api_types::command::IdempotencyKey::parse(Some(&key)).is_ok());
        }
    }

    #[test]
    fn a_refused_token_says_log_in_again_and_other_refusals_are_the_apis() {
        let refused = |status, code| -> Reply<()> {
            Err(Refused {
                status,
                body: voidforge_api_types::error::ErrorBody {
                    code,
                    message: "m".to_owned(),
                },
            })
        };
        let saved = |expires_at| Saved {
            issuer: "http://localhost:5598/dex".to_owned(),
            expires_at,
        };
        // Within its saved lifetime, a refused login names its issuer and doesn't claim expiry.
        let valid = saved(session::now_unix_s() + 3_600);
        let Err(Failure::LogInAgain(m)) = answer(refused(401, ErrorCode::InvalidToken), &valid)
        else {
            panic!("a 401 invalid_token isn't log in again");
        };
        assert_eq!(
            m,
            format!(
                "the API refused your saved login (it may be from another dex: you logged in at \
                 http://localhost:5598/dex); {LOG_IN_AGAIN}"
            )
        );
        // Past it, the login has expired.
        let past = saved(session::now_unix_s() - 1);
        let Err(Failure::LogInAgain(m)) = answer(refused(401, ErrorCode::MissingToken), &past)
        else {
            panic!("a 401 missing_token isn't log in again");
        };
        assert_eq!(m, format!("your login has expired; {LOG_IN_AGAIN}"));
        assert!(matches!(
            answer(refused(422, ErrorCode::UnknownFleet), &valid),
            Err(Failure::Refused(m)) if m.contains("unknown_fleet")
        ));
    }
}
