//! Logging in through dex (D-052). The end-to-end scenario uses the password grant for the static
//! test users, which only the CI dex config enables (Technical Plan, Auth), as the public
//! `voidforge-cli` client: no secret, and the ID token's audience is `voidforge-cli`.
//!
//! P3.7 adds the device code grant (RFC 8628) beside it, for the same client: how a person logs in
//! from a terminal (D-053). The person sees only the verification URL and the user code; the device
//! code, the token and the subject are never printed or logged.

use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;

use crate::Failure;

/// The dex client the bots and the terminal client log in as (deploy/dex/config.yaml).
pub const CLIENT_ID: &str = "voidforge-cli";

/// A static test user from deploy/dex/config.ci.yaml. TEST-ONLY credentials.
#[derive(Debug, Clone, Copy)]
pub struct TestUser {
    /// The name the scenario's log uses.
    pub name: &'static str,
    /// The dex login (an email address).
    pub email: &'static str,
    /// The password.
    pub password: &'static str,
}

/// alice (TEST-ONLY).
pub const ALICE: TestUser = TestUser {
    name: "alice",
    email: "alice@voidforge.test",
    password: "alice-local-test-password",
};

/// bob (TEST-ONLY).
pub const BOB: TestUser = TestUser {
    name: "bob",
    email: "bob@voidforge.test",
    password: "bob-local-test-password",
};

/// dex's token response: only the ID token matters.
#[derive(Deserialize)]
struct TokenReply {
    id_token: String,
}

/// dex's error response (RFC 6749, 5.2).
#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// The device code grant's type.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// How much longer to wait after a `slow_down` (RFC 8628, 3.5).
const SLOW_DOWN: Duration = Duration::from_secs(5);

/// dex's device authorization response (RFC 8628, 3.2).
#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

/// RFC 8628, 3.2: 5 seconds when the server doesn't say.
fn default_interval() -> u64 {
    5
}

/// The token response's parts a terminal login keeps.
#[derive(Deserialize)]
struct DeviceTokenReply {
    id_token: String,
    expires_in: u64,
}

/// What the person must do to approve a login: shown on stderr, and nothing secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The URL with the code filled in, when dex offers one.
    pub complete: Option<String>,
    /// The plain verification URL.
    pub uri: String,
    /// The user code to enter there.
    pub user_code: String,
    /// How long the code is valid.
    pub expires_in: Duration,
}

/// A login's result: the ID token, and how long dex says it lasts.
pub struct IdToken {
    /// The token. A credential: never print or log it.
    pub token: String,
    /// Its lifetime from now, as dex's token response gives it.
    pub expires_in: Duration,
}

impl std::fmt::Debug for IdToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdToken")
            .field("token", &"<hidden>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Logs in through dex's device code flow at `issuer` as the public `voidforge-cli` client:
/// requests a device code, hands `show` what the person must do, then polls the token endpoint at
/// dex's interval (5 s more after each `slow_down`) until the person approves, declines or the code
/// expires.
///
/// # Errors
/// [`Failure::Declined`] for `access_denied`; [`Failure::CodeExpired`] for `expired_token` or when
/// the code's `expires_in` passes; [`Failure::Login`] for any other refusal; [`Failure::Http`] when
/// dex can't be reached or answers outside the protocol.
pub async fn device(
    http: &reqwest::Client,
    issuer: &str,
    show: impl FnOnce(&Prompt),
) -> Result<IdToken, Failure> {
    let url = format!("{issuer}/device/code");
    let (status, text) = form_post(
        http,
        &url,
        &format!("client_id={CLIENT_ID}&scope=openid%20email"),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Err(Failure::Login(format!(
            "dex refused to start a login (HTTP {status}): {}",
            describe(&text)
        )));
    }
    let code: DeviceCode = serde_json::from_str(&text)
        .map_err(|e| Failure::Http(format!("dex at {url}: not a device code response: {e}")))?;
    show(&Prompt {
        complete: code.verification_uri_complete.clone(),
        uri: code.verification_uri.clone(),
        user_code: code.user_code.clone(),
        expires_in: Duration::from_secs(code.expires_in),
    });
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval);
    let token_url = format!("{issuer}/token");
    let body = format!(
        "grant_type={}&device_code={}&client_id={CLIENT_ID}",
        form_encode(DEVICE_GRANT),
        form_encode(&code.device_code),
    );
    loop {
        if Instant::now() + interval >= deadline {
            return Err(Failure::CodeExpired(
                "the login code expired before it was approved".to_owned(),
            ));
        }
        tokio::time::sleep(interval).await;
        let (status, text) = form_post(http, &token_url, &body).await?;
        if (200..300).contains(&status) {
            let reply: DeviceTokenReply = serde_json::from_str(&text).map_err(|e| {
                Failure::Http(format!(
                    "dex at {token_url}: no ID token in its answer: {e}"
                ))
            })?;
            return Ok(IdToken {
                token: reply.id_token,
                expires_in: Duration::from_secs(reply.expires_in),
            });
        }
        match serde_json::from_str::<TokenError>(&text)
            .map(|e| e.error)
            .as_deref()
        {
            Ok("authorization_pending") => {}
            Ok("slow_down") => interval += SLOW_DOWN,
            Ok("access_denied") => {
                return Err(Failure::Declined(
                    "the login was declined in the browser".to_owned(),
                ));
            }
            Ok("expired_token") => {
                return Err(Failure::CodeExpired(
                    "the login code expired before it was approved".to_owned(),
                ));
            }
            _ => {
                return Err(Failure::Login(format!(
                    "dex refused the login (HTTP {status}): {}",
                    describe(&text)
                )));
            }
        }
    }
}

/// A form POST to dex: the status and the body's text.
async fn form_post(
    http: &reqwest::Client,
    url: &str,
    body: &str,
) -> Result<(u16, String), Failure> {
    let response = http
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.to_owned())
        .send()
        .await
        .map_err(|e| Failure::Http(format!("dex at {url}: {e}")))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|e| Failure::Http(format!("dex at {url}: {e}")))?;
    Ok((status, text))
}

/// dex's error and description, or the body when it isn't an OAuth error. dex's answers never
/// echo the device code or a token.
fn describe(text: &str) -> String {
    match serde_json::from_str::<TokenError>(text) {
        Ok(TokenError {
            error,
            error_description: Some(d),
        }) => format!("{error}: {d}"),
        Ok(TokenError { error, .. }) => error,
        Err(_) => text.chars().take(200).collect(),
    }
}

/// An ID token for `user` from dex's password grant at `issuer`.
///
/// # Errors
/// [`Failure::Login`] if dex refuses (the hint names the CI dex config when the grant is off),
/// [`Failure::Http`] if dex can't be reached.
pub async fn password(
    http: &reqwest::Client,
    issuer: &str,
    user: TestUser,
) -> Result<String, Failure> {
    let body = format!(
        "grant_type=password&username={}&password={}&client_id={CLIENT_ID}&scope=openid%20email",
        form_encode(user.email),
        form_encode(user.password),
    );
    let url = format!("{issuer}/token");
    let response = http
        .post(&url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| Failure::Http(format!("dex at {url}: {e}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| Failure::Http(format!("dex at {url}: {e}")))?;
    if status.is_success() {
        let reply: TokenReply = serde_json::from_str(&text).map_err(|e| {
            Failure::Login(format!(
                "{}: dex answered without an ID token: {e}",
                user.name
            ))
        })?;
        return Ok(reply.id_token);
    }
    let code = serde_json::from_str::<TokenError>(&text)
        .ok()
        .map(|e| e.error);
    let hint = if code.as_deref() == Some("unsupported_grant_type") {
        "; the password grant is off: start the stack with the CI dex config, \
         `DEX_CONFIG=config.ci.yaml just up`"
    } else {
        ""
    };
    Err(Failure::Login(format!(
        "{}: dex refused the password grant (HTTP {status}): {text}{hint}",
        user.name
    )))
}

/// `application/x-www-form-urlencoded`: unreserved bytes as they are, every other byte as `%XX`.
fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_escapes_everything_but_unreserved_bytes() {
        assert_eq!(
            form_encode("alice@voidforge.test"),
            "alice%40voidforge.test"
        );
        assert_eq!(form_encode("a b&c=d+é"), "a%20b%26c%3Dd%2B%C3%A9");
        assert_eq!(form_encode("A-z0._~"), "A-z0._~");
    }

    /// A dex stand-in for the device flow: answers `/device/code` with `code`, then each `/token`
    /// poll with the next of `tokens` (status and body), recording every request's path, body and
    /// arrival time. Faithful to what dex v2.46 was measured to send (D-053).
    struct StandIn {
        issuer: String,
        requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String, std::time::Instant)>>>,
    }

    impl StandIn {
        fn start(code: &str, tokens: Vec<(u16, String)>) -> Self {
            use std::io::{BufRead as _, BufReader, Read as _};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let issuer = format!("http://{}/dex", listener.local_addr().unwrap());
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let seen = requests.clone();
            let code = code.to_owned();
            std::thread::spawn(move || {
                let mut tokens = tokens.into_iter();
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        if header == "\r\n" || header.is_empty() {
                            break;
                        }
                        if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = v.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    seen.lock().unwrap().push((
                        path.clone(),
                        String::from_utf8(body).unwrap(),
                        std::time::Instant::now(),
                    ));
                    let (status, reply) = if path.ends_with("/device/code") {
                        (200, code.clone())
                    } else {
                        tokens.next().unwrap_or((500, "{}".to_owned()))
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
                }
            });
            Self { issuer, requests }
        }

        /// The token polls' arrival times.
        fn polls(&self) -> Vec<std::time::Instant> {
            let seen = self.requests.lock().unwrap();
            seen.iter()
                .filter(|(p, _, _)| p.ends_with("/token"))
                .map(|(_, _, t)| *t)
                .collect()
        }
    }

    const SECRET_DEVICE_CODE: &str = "secret-device-code-0123456789";

    fn code(expires_in: u64, interval: u64) -> String {
        format!(
            r#"{{"device_code":"{SECRET_DEVICE_CODE}","user_code":"ABCD-EFGH","verification_uri":"http://dex/device","verification_uri_complete":"http://dex/device?user_code=ABCD-EFGH","expires_in":{expires_in},"interval":{interval}}}"#
        )
    }

    fn error(name: &str) -> (u16, String) {
        (400, format!(r#"{{"error":"{name}"}}"#))
    }

    fn ok_token() -> (u16, String) {
        (
            200,
            r#"{"access_token":"a","token_type":"bearer","expires_in":86400,"id_token":"the.id.token"}"#
                .to_owned(),
        )
    }

    async fn run(stand_in: &StandIn) -> (Result<IdToken, Failure>, Option<Prompt>) {
        let mut shown = None;
        let result = device(&reqwest::Client::new(), &stand_in.issuer, |p| {
            shown = Some(p.clone());
        })
        .await;
        (result, shown)
    }

    #[tokio::test]
    async fn pending_then_approved_gives_the_token_and_shows_only_the_url_and_code() {
        let stand_in = StandIn::start(
            &code(300, 1),
            vec![error("authorization_pending"), ok_token()],
        );
        let (result, shown) = run(&stand_in).await;
        let token = result.unwrap();
        assert_eq!(token.token, "the.id.token");
        assert_eq!(token.expires_in, Duration::from_secs(86_400));
        assert_eq!(
            shown.unwrap(),
            Prompt {
                complete: Some("http://dex/device?user_code=ABCD-EFGH".to_owned()),
                uri: "http://dex/device".to_owned(),
                user_code: "ABCD-EFGH".to_owned(),
                expires_in: Duration::from_secs(300),
            }
        );
        // The public client: no secret is ever sent, and the polls carry the device code.
        let seen = stand_in.requests.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(
            seen.iter().all(|(_, b, _)| !b.contains("client_secret")),
            "{seen:?}"
        );
        assert!(seen[0].1.contains("client_id=voidforge-cli"));
        assert!(seen[1].1.contains(SECRET_DEVICE_CODE));
        // The Debug form never shows the token.
        assert!(!format!("{token:?}").contains("the.id.token"));
    }

    #[tokio::test]
    async fn polls_wait_the_interval_and_slow_down_adds_five_seconds() {
        let stand_in = StandIn::start(
            &code(300, 1),
            vec![
                error("authorization_pending"),
                error("slow_down"),
                error("authorization_pending"),
                ok_token(),
            ],
        );
        assert!(run(&stand_in).await.0.is_ok());
        let polls = stand_in.polls();
        let gaps: Vec<_> = polls.windows(2).map(|w| w[1] - w[0]).collect();
        // 1 s, then after the slow_down 1 + 5 = 6 s, and 6 s again: never sooner.
        assert_eq!(gaps.len(), 3);
        assert!(
            gaps[0] >= Duration::from_secs(1) && gaps[0] < Duration::from_secs(2),
            "{gaps:?}"
        );
        for gap in &gaps[1..] {
            assert!(
                *gap >= Duration::from_secs(6) && *gap < Duration::from_secs(7),
                "{gaps:?}"
            );
        }
    }

    #[tokio::test]
    async fn declined_and_expired_are_their_own_failures() {
        let declined = StandIn::start(&code(300, 0), vec![error("access_denied")]);
        assert!(matches!(run(&declined).await.0, Err(Failure::Declined(_))));
        let expired = StandIn::start(&code(300, 0), vec![error("expired_token")]);
        assert!(matches!(
            run(&expired).await.0,
            Err(Failure::CodeExpired(_))
        ));
        let other = StandIn::start(
            &code(300, 0),
            vec![(
                400,
                r#"{"error":"invalid_request","error_description":"Invalid Device code."}"#
                    .to_owned(),
            )],
        );
        let Err(Failure::Login(message)) = run(&other).await.0 else {
            panic!("an unknown error isn't a login failure");
        };
        assert!(
            message.contains("invalid_request: Invalid Device code."),
            "{message}"
        );
        assert!(!message.contains(SECRET_DEVICE_CODE), "{message}");
    }

    #[tokio::test]
    async fn the_code_s_lifetime_bounds_the_wait() {
        // Always pending: 2 s of life at a 1 s interval allows one poll, then it's expired.
        let stand_in = StandIn::start(
            &code(2, 1),
            std::iter::repeat_with(|| error("authorization_pending"))
                .take(10)
                .collect(),
        );
        let started = std::time::Instant::now();
        assert!(matches!(
            run(&stand_in).await.0,
            Err(Failure::CodeExpired(_))
        ));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(stand_in.polls().len(), 1);
    }
}
