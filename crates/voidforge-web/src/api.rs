//! The one API call P4.1 makes (D-057): `GET /players/me`, proving the API accepts the web
//! app's token end to end. P4.2 replaces this with the typed client.

use std::time::Duration;

use voidforge_api_types::error::{ErrorBody, ErrorCode};
use voidforge_api_types::view::PlayerView;

use crate::config::Secret;

/// How long one API call may take.
pub const API_TIMEOUT: Duration = Duration::from_secs(10);

/// What `/players/me` answered.
#[derive(Debug)]
pub enum MeAnswer {
    /// 200: the player.
    Player(PlayerView),
    /// 401: the token isn't accepted, so the login is over.
    Unauthorized,
    /// Another error with its code.
    Refused(ErrorCode),
    /// No answer, or one that isn't the contract's.
    Failed(&'static str),
}

/// The HTTP client for the API: bounded, and following no redirects.
///
/// # Errors
/// reqwest's, if the client can't be built.
pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(API_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Calls `GET /players/me` with the ID token.
pub async fn player_me(http: &reqwest::Client, api_url: &str, token: &Secret) -> MeAnswer {
    let response = match http
        .get(format!("{api_url}/players/me"))
        .bearer_auth(token.expose())
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return MeAnswer::Failed("the game server can't be reached"),
    };
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return MeAnswer::Unauthorized;
    }
    if status == reqwest::StatusCode::OK {
        return match response.json::<PlayerView>().await {
            Ok(v) => MeAnswer::Player(v),
            Err(_) => MeAnswer::Failed("the game server's answer isn't the expected player view"),
        };
    }
    match response.json::<ErrorBody>().await {
        Ok(body) => MeAnswer::Refused(body.code),
        Err(_) => MeAnswer::Failed("the game server answered an error without the error body"),
    }
}
