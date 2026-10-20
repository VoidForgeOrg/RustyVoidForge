//! Logging in through dex (D-052). The end-to-end scenario uses the password grant for the static
//! test users, which only the CI dex config enables (Technical Plan, Auth), as the public
//! `voidforge-cli` client: no secret, and the ID token's audience is `voidforge-cli`. P3.7 adds the
//! device code grant beside it, for the same client.

use serde::Deserialize;

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
}
