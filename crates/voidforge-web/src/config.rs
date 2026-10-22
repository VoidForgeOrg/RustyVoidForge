//! The web app's configuration (P4.1, D-057), in the style of `serve`'s (D-050): flags with
//! environment fallbacks, a default only where it's safe, and refusals before anything starts.
//!
//! The client secret is read from the environment only (`VOIDFORGE_WEB_CLIENT_SECRET`), never
//! from a flag, so it can't show up in a process listing. It's required, and nothing prints it.

use std::fmt;
use std::net::SocketAddr;

use clap::Parser;

/// The local stack's issuer (D-015).
pub const DEFAULT_ISSUER: &str = "http://localhost:5556/dex";
/// dex's client for the web app (`deploy/dex/config.yaml`).
pub const DEFAULT_CLIENT_ID: &str = "voidforge-web";
/// The redirect dex has registered for the web app, by exact string.
pub const DEFAULT_REDIRECT_URL: &str = "http://localhost:8081/callback";
/// The environment variable holding the client secret.
pub const SECRET_ENV: &str = "VOIDFORGE_WEB_CLIENT_SECRET";

/// The command line.
#[derive(Debug, Parser)]
#[command(
    name = "voidforge-web",
    about = "Voidforge's browser frontend",
    version
)]
pub struct Args {
    /// For `just trace-check` (D-064): record one `telemetry_check` span, print `trace_id=<hex>`
    /// on stdout when exporting, and exit, reading no other setting.
    #[arg(long, hide = true)]
    pub telemetry_check: bool,
    /// For a container's healthcheck (P5.2, D-065): GET this URL (the web's own `/`), bounded at
    /// 2 s, and exit 0 only on a 200, otherwise 1; it reads no other setting and starts no
    /// telemetry.
    #[arg(long, hide = true, value_name = "URL")]
    pub health_check: Option<String>,
    /// The OIDC issuer; the web app reads its endpoints from the issuer's discovery document.
    #[arg(long, env = "VOIDFORGE_WEB_ISSUER", default_value = DEFAULT_ISSUER)]
    pub issuer: String,
    /// The OIDC client ID.
    #[arg(long, env = "VOIDFORGE_WEB_CLIENT_ID", default_value = DEFAULT_CLIENT_ID)]
    pub client_id: String,
    /// Where dex sends the browser back; it must be registered with dex exactly, and its path
    /// must be /callback.
    #[arg(long, env = "VOIDFORGE_WEB_REDIRECT_URL", default_value = DEFAULT_REDIRECT_URL)]
    pub redirect_url: String,
    /// The address to listen on.
    #[arg(long, env = "VOIDFORGE_WEB_BIND", default_value = "127.0.0.1:8081")]
    pub bind: SocketAddr,
    /// The game API's base URL.
    #[arg(
        long,
        env = "VOIDFORGE_WEB_API_URL",
        default_value = "http://localhost:8080"
    )]
    pub api_url: String,
    /// Mark the session cookie Secure. Turn it on whenever the web app is served over https.
    #[arg(long, env = "VOIDFORGE_WEB_SECURE_COOKIE")]
    pub secure_cookie: bool,
}

/// A value that must never be printed: its `Debug` says only that it's hidden.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The value, for the one place that sends it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

/// The checked configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// The issuer, exactly as tokens must name it.
    pub issuer: String,
    /// The client ID.
    pub client_id: String,
    /// The client secret.
    pub client_secret: Secret,
    /// The redirect URL.
    pub redirect_url: String,
    /// The listen address.
    pub bind: SocketAddr,
    /// The API's base URL, without a trailing slash.
    pub api_url: String,
    /// Whether the session cookie is Secure.
    pub secure_cookie: bool,
}

/// A configuration the web app refuses to start with.
#[derive(Debug, thiserror::Error)]
#[error("{setting}: {reason}")]
pub struct ConfigError {
    /// The flag or variable.
    pub setting: &'static str,
    /// What's wrong with it.
    pub reason: &'static str,
}

fn bad(setting: &'static str, reason: &'static str) -> ConfigError {
    ConfigError { setting, reason }
}

/// `http://` or `https://` followed by a host.
fn is_http_url(url: &str) -> bool {
    url.strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

impl Args {
    /// Checks the flags and takes the secret.
    ///
    /// # Errors
    /// [`ConfigError`] for a non-HTTP issuer, redirect or API URL, a redirect whose path isn't
    /// /callback, an empty client ID, or a missing or empty secret.
    pub fn into_config(self, secret: Option<String>) -> Result<Config, ConfigError> {
        if !is_http_url(&self.issuer) {
            return Err(bad("--issuer", "must be an http:// or https:// URL"));
        }
        if self.client_id.is_empty() {
            return Err(bad("--client-id", "must not be empty"));
        }
        if !is_http_url(&self.redirect_url) {
            return Err(bad("--redirect-url", "must be an http:// or https:// URL"));
        }
        let after_scheme = self
            .redirect_url
            .split_once("://")
            .map_or("", |(_, rest)| rest);
        if after_scheme.find('/').map(|i| &after_scheme[i..]) != Some("/callback") {
            return Err(bad("--redirect-url", "its path must be /callback"));
        }
        if !is_http_url(&self.api_url) {
            return Err(bad("--api-url", "must be an http:// or https:// URL"));
        }
        let secret = secret
            .filter(|s| !s.is_empty())
            .ok_or(bad(SECRET_ENV, "must be set to the client secret"))?;
        Ok(Config {
            issuer: self.issuer,
            client_id: self.client_id,
            client_secret: Secret::new(secret),
            redirect_url: self.redirect_url,
            bind: self.bind,
            api_url: self.api_url.trim_end_matches('/').to_owned(),
            secure_cookie: self.secure_cookie,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["voidforge-web"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("parses")
    }

    #[test]
    fn defaults_match_the_local_stack() {
        let c = args(&[]).into_config(Some("s".to_owned())).expect("valid");
        assert_eq!(c.issuer, "http://localhost:5556/dex");
        assert_eq!(c.client_id, "voidforge-web");
        assert_eq!(c.redirect_url, "http://localhost:8081/callback");
        assert_eq!(c.bind.to_string(), "127.0.0.1:8081");
        assert_eq!(c.api_url, "http://localhost:8080");
        assert!(!c.secure_cookie);
    }

    #[test]
    fn the_secret_is_required_and_never_printed() {
        for missing in [None, Some(String::new())] {
            let e = args(&[]).into_config(missing).expect_err("refused");
            assert_eq!(e.setting, SECRET_ENV);
        }
        let c = args(&[])
            .into_config(Some("hunter2-secret".to_owned()))
            .expect("valid");
        assert!(!format!("{c:?}").contains("hunter2"), "{c:?}");
    }

    #[test]
    fn there_is_no_secret_flag() {
        let argv = ["voidforge-web", "--client-secret", "x"];
        assert!(Args::try_parse_from(argv).is_err());
    }

    #[test]
    fn bad_urls_are_refused() {
        let cases: [(&[&str], &str); 6] = [
            (&["--issuer", "localhost:5556/dex"], "--issuer"),
            (&["--issuer", "http://"], "--issuer"),
            (&["--client-id", ""], "--client-id"),
            (
                &["--redirect-url", "http://localhost:8081/cb"],
                "--redirect-url",
            ),
            (
                &["--redirect-url", "http://localhost:8081"],
                "--redirect-url",
            ),
            (&["--api-url", "ftp://localhost"], "--api-url"),
        ];
        for (flags, setting) in cases {
            let e = args(flags)
                .into_config(Some("s".to_owned()))
                .expect_err("refused");
            assert_eq!(e.setting, setting, "{flags:?}");
        }
    }

    #[test]
    fn the_api_url_loses_its_trailing_slash() {
        let c = args(&["--api-url", "http://localhost:8080/"])
            .into_config(Some("s".to_owned()))
            .expect("valid");
        assert_eq!(c.api_url, "http://localhost:8080");
    }
}
