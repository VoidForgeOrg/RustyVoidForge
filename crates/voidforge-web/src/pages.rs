//! The pages (P4.1, D-057): plain HTML from maud, browser default styles, no script.

use maud::{DOCTYPE, Markup, html};
use voidforge_api_types::error::ErrorCode;

use crate::api::MeAnswer;
use crate::session::LoggedIn;

/// A line shown above the logged-out Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The login expired, or the API stopped accepting it.
    Expired,
}

/// The advisor's sentence for a full universe (P4.1 A1).
pub const UNIVERSE_FULL: &str =
    "This universe is full: there's no free homeworld left, so you can't join it.";
/// The advisor's sentence for an expired login (P4.1 A4).
pub const EXPIRED: &str = "Your login has expired. Log in again.";

fn layout(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " – Voidforge" }
            }
            body { (body) }
        }
    }
}

/// The public short form of a player's ID (phase4-ux Q1): `player` and the UUID's last 8 hex
/// digits, since a UUIDv7 starts with its creation time.
#[must_use]
pub fn short_name(player_id: &str) -> String {
    let tail = player_id
        .char_indices()
        .rev()
        .nth(7)
        .map_or(player_id, |(i, _)| &player_id[i..]);
    format!("player {tail}")
}

/// Home, logged out.
#[must_use]
pub fn home_logged_out(notice: Option<Notice>, login_href: &str) -> Markup {
    layout(
        "Home",
        html! {
            h1 { "Voidforge" }
            @if notice == Some(Notice::Expired) {
                p { strong { (EXPIRED) } }
            }
            p { a href=(login_href) { "Log in" } }
        },
    )
}

fn answer_line(code: ErrorCode) -> String {
    match code {
        ErrorCode::NoFreeHomeworld => UNIVERSE_FULL.to_owned(),
        ErrorCode::RateLimited => "Too many requests just now. Refresh in a moment.".to_owned(),
        ErrorCode::Unavailable | ErrorCode::Busy => {
            "The game server is busy or starting. Refresh in a moment.".to_owned()
        }
        other => format!(
            "The game server refused: {}.",
            serde_json::to_value(other)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "an unknown code".to_owned())
        ),
    }
}

/// Home, logged in, with what `/players/me` answered.
#[must_use]
pub fn home_logged_in(login: &LoggedIn, me: &MeAnswer) -> Markup {
    let who = login
        .email
        .as_deref()
        .or(login.name.as_deref())
        .unwrap_or("you");
    layout(
        "Home",
        html! {
            h1 { "Voidforge" }
            p { "Logged in as " strong { (who) } "." }
            @match me {
                MeAnswer::Player(v) => {
                    p {
                        "You appear to others as "
                        span title=(v.player_id.as_str()) { (short_name(v.player_id.as_str())) }
                        "."
                    }
                    p { "Score: " (v.score) }
                }
                MeAnswer::Refused(code) => { p { (answer_line(*code)) } }
                MeAnswer::Failed(why) => { p { "The game can't be shown: " (why) ". Refresh in a moment." } }
                MeAnswer::Unauthorized => {}
            }
            p { a href="/" { "Refresh" } }
            form method="post" action="/logout" {
                button type="submit" { "Log out" }
            }
        },
    )
}

/// Why a login was refused, in words for the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The callback has no state, or one this browser didn't start, or one already used.
    UnknownAttempt,
    /// dex sent the browser back with an error.
    Provider,
    /// The callback has no code.
    NoCode,
    /// The code couldn't be exchanged, or the answer failed a check.
    Rejected,
    /// The login provider can't be reached to check the answer.
    Unavailable,
}

/// The page for a refused login: always 400 or 503, never a session.
#[must_use]
pub fn refused(why: Refusal) -> Markup {
    let text = match why {
        Refusal::UnknownAttempt => {
            "This login link isn't one this browser started, or it was already used."
        }
        Refusal::Provider => "The login provider didn't complete the login.",
        Refusal::NoCode => "The login provider's answer is incomplete.",
        Refusal::Rejected => "The login provider's answer couldn't be verified.",
        Refusal::Unavailable => "The login provider can't be reached right now.",
    };
    layout(
        "Login failed",
        html! {
            h1 { "Login failed" }
            p { (text) " You are not logged in." }
            p { a href="/login" { "Log in again" } }
        },
    )
}

/// A page for a failure of the web app itself.
#[must_use]
pub fn internal() -> Markup {
    layout(
        "Error",
        html! {
            h1 { "Something went wrong" }
            p { "The web app couldn't complete this request. " a href="/" { "Home" } }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_short_name_is_the_last_eight_hex_digits() {
        assert_eq!(
            short_name("01890a5d-ac96-774b-bcce-b302099a8057"),
            "player 099a8057"
        );
        assert_eq!(short_name("abc"), "player abc");
    }

    #[test]
    fn a_full_universe_is_a_sentence_not_a_code() {
        assert_eq!(answer_line(ErrorCode::NoFreeHomeworld), UNIVERSE_FULL);
        assert!(answer_line(ErrorCode::InvalidToken).contains("invalid_token"));
    }

    #[test]
    fn logout_is_a_post_form() {
        let login = LoggedIn {
            id_token: crate::config::Secret::new("t".to_owned()),
            email: Some("alice@voidforge.test".to_owned()),
            name: None,
            exp: 1,
        };
        let page = home_logged_in(&login, &MeAnswer::Failed("x")).into_string();
        assert!(
            page.contains(r#"<form method="post" action="/logout">"#),
            "{page}"
        );
        assert!(!page.contains(r#"href="/logout""#), "{page}");
    }

    #[test]
    fn a_name_is_escaped() {
        let login = LoggedIn {
            id_token: crate::config::Secret::new("t".to_owned()),
            email: None,
            name: Some("<script>x</script>".to_owned()),
            exp: 1,
        };
        let page = home_logged_in(&login, &MeAnswer::Failed("x")).into_string();
        assert!(!page.contains("<script>"), "{page}");
    }
}
