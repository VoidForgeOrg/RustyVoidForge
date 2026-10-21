//! The pages: plain HTML from maud, browser default styles, no script. P4.1 (D-057) brought the
//! login's pages; P4.2 (D-059) the frame every page shares (phase4-ux §2), Home's data (§4.1),
//! and the pages for a failed read: the retry page (§5.5) and the full universe (§4.1).

use maud::{DOCTYPE, Markup, html};
use voidforge_api_types::view::PlayerView;

use crate::messages::Message;
use crate::session::LoggedIn;

/// A line shown above the logged-out Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The login expired, or the API stopped accepting it.
    Expired,
}

/// The advisor's sentence for an expired login (P4.1 A4; kept for P4.2 by its A1).
pub const EXPIRED: &str = "Your login has expired. Log in again.";

/// What every page's frame shows: the fast-rules line, and for a logged-in viewer who they are.
#[derive(Debug, Clone, Copy, Default)]
pub struct Frame<'a> {
    /// The rules are `v0-fast` (§2). False when `/rules` failed: the line is left out.
    pub fast: bool,
    /// The viewer, when logged in.
    pub login: Option<&'a LoggedIn>,
    /// The viewer's player, when `/players/me` answered.
    pub me: Option<&'a PlayerView>,
}

fn layout(title: &str, frame: Frame<'_>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " – Voidforge" }
            }
            body {
                @if let Some(login) = frame.login {
                    p {
                        strong { "Voidforge" } " " a href="/" { "Home" }
                        @if let Some(me) = frame.me {
                            " · You: " (player(me.player_id.as_str())) " · score " (thousands(me.score))
                        }
                    }
                    p { "Logged in as " strong { (who(login)) } "." }
                    form method="post" action="/logout" {
                        button type="submit" { "Log out" }
                    }
                    hr;
                }
                @if frame.fast {
                    p { strong { "Fast test rules:" } " an hour of play takes a second. These timings aren't the real game's." }
                }
                (body)
            }
        }
    }
}

fn who(login: &LoggedIn) -> &str {
    login
        .email
        .as_deref()
        .or(login.name.as_deref())
        .unwrap_or("you")
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

/// A player's short name, with the full ID in a hover (§3).
fn player(player_id: &str) -> Markup {
    html! { span title=(player_id) { (short_name(player_id)) } }
}

/// A whole number with thousands separated: `3,450`, `-1,200` (§3).
#[must_use]
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A view's time as `HH:MM:SS UTC` (§3). api-types checked its shape when it was read.
#[must_use]
pub fn clock(timestamp: &str) -> String {
    timestamp
        .get(11..19)
        .map_or_else(|| timestamp.to_owned(), |t| format!("{t} UTC"))
}

/// Home, logged out.
#[must_use]
pub fn home_logged_out(notice: Option<Notice>, login_href: &str, fast: bool) -> Markup {
    layout(
        "Home",
        Frame {
            fast,
            ..Frame::default()
        },
        html! {
            h1 { "Voidforge" }
            @if notice == Some(Notice::Expired) {
                p { strong { (EXPIRED) } }
            }
            p { "A persistent space strategy game. Log in to play." }
            p { a href=(login_href) { "Log in" } }
        },
    )
}

fn planet_link(id: u64) -> Markup {
    html! { a href={ "/planets/" (id) } { "Planet #" (id) } }
}

/// Home, logged in, with the player (§4.1).
#[must_use]
pub fn home_logged_in(login: &LoggedIn, me: &PlayerView, fast: bool) -> Markup {
    let home = me.homeworld_id.0;
    let mut others: Vec<u64> = me
        .planet_ids
        .iter()
        .map(|p| p.0)
        .filter(|p| *p != home)
        .collect();
    others.sort_unstable();
    let owns_home = me.planet_ids.iter().any(|p| p.0 == home);
    let count = others.len() + usize::from(owns_home);
    layout(
        "Home",
        Frame {
            fast,
            login: Some(login),
            me: Some(me),
        },
        html! {
            h1 { "Your empire" }
            @if owns_home && others.is_empty() {
                p { "Welcome. Planet #" (home) " is your homeworld: a Drill mines iron ore and a Refinery turns it into ingots. Build more, then a Shipyard to make ships." }
            }
            p { "Score: " (thousands(me.score)) " (current)" }
            p { "Score as of " (clock(me.as_of.as_str())) "." }
            p { "Homeworld: " (planet_link(home)) }
            p {
                "Your planets (" (count) "): "
                @if owns_home {
                    (planet_link(home)) " (homeworld)"
                }
                @for (i, p) in others.iter().enumerate() {
                    @if owns_home || i > 0 { " · " }
                    (planet_link(*p))
                }
            }
            p { "Next: open a planet to build, or Fleets to send ships." }
            p { a href="/" { "Refresh" } }
        },
    )
}

/// The code beside a copy: ` (busy)`, or nothing for the web's own states.
fn code(m: &Message) -> Markup {
    html! {
        @if let Some(c) = m.code {
            " (" code { (c) } ")"
        }
    }
}

/// §5.5 for a page's read: the copy and code, and Try again as a link to the same page.
#[must_use]
pub fn retry_page(frame: Frame<'_>, m: &Message, again: &str) -> Markup {
    layout(
        "Couldn't load this page",
        frame,
        html! {
            h1 { "Couldn't load this page" }
            p { (m.copy) (code(m)) }
            p { a href=(again) { "Try again" } }
        },
    )
}

/// §4.1: the universe is full, so this account can't join: the heading, §6.1's copy with its
/// code (it doesn't repeat the heading: QA's F2), and Log out as the only action.
#[must_use]
pub fn universe_full(fast: bool, m: &Message) -> Markup {
    layout(
        "The universe is full",
        Frame {
            fast,
            ..Frame::default()
        },
        html! {
            h1 { "The universe is full" }
            p { (m.copy) (code(m)) }
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

/// The page for a refused login: always 400 or 503, never a new login. `logged_in` says whether
/// the browser still has the login it had before.
#[must_use]
pub fn refused(why: Refusal, logged_in: bool) -> Markup {
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
        Frame::default(),
        html! {
            h1 { "Login failed" }
            @if logged_in {
                p { (text) " You are still logged in from before." }
                p { a href="/" { "Home" } }
            } @else {
                p { (text) " You are not logged in." }
                p { a href="/login" { "Log in again" } }
            }
        },
    )
}

/// The page when the session store is full (R1-1): try again later.
#[must_use]
pub fn busy() -> Markup {
    layout(
        "Busy",
        Frame::default(),
        html! {
            h1 { "Too many logins at once" }
            p { "The web app is holding as many login attempts as it can. Try again in a few minutes. " a href="/" { "Home" } }
        },
    )
}

/// A page for a failure of the web app itself.
#[must_use]
pub fn internal() -> Markup {
    layout(
        "Error",
        Frame::default(),
        html! {
            h1 { "Something went wrong" }
            p { "The web app couldn't complete this request. " a href="/" { "Home" } }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(name: Option<&str>) -> LoggedIn {
        LoggedIn {
            id_token: crate::config::Secret::new("t".to_owned()),
            email: name.is_none().then(|| "alice@voidforge.test".to_owned()),
            name: name.map(str::to_owned),
            exp: 1,
        }
    }

    fn me(planets: &[u64]) -> PlayerView {
        serde_json::from_value(serde_json::json!({
            "player_id": "01890a5d-ac96-774b-bcce-b302099a8057", "homeworld_id": 7,
            "planet_ids": planets, "score": 3450,
            "as_of": "2026-10-20T18:04:12.000Z", "valid_until": null,
        }))
        .unwrap()
    }

    #[test]
    fn the_short_name_is_the_last_eight_hex_digits() {
        assert_eq!(
            short_name("01890a5d-ac96-774b-bcce-b302099a8057"),
            "player 099a8057"
        );
        assert_eq!(short_name("abc"), "player abc");
    }

    #[test]
    fn numbers_and_times_are_formatted() {
        for (n, s) in [
            (0, "0"),
            (999, "999"),
            (1000, "1,000"),
            (3450, "3,450"),
            (-1_234_567, "-1,234,567"),
            (i64::MIN, "-9,223,372,036,854,775,808"),
        ] {
            assert_eq!(thousands(n), s);
        }
        assert_eq!(clock("2026-10-20T18:04:12.000Z"), "18:04:12 UTC");
    }

    #[test]
    fn logout_is_a_post_form() {
        let page = home_logged_in(&login(None), &me(&[7]), false).into_string();
        assert!(
            page.contains(r#"<form method="post" action="/logout">"#),
            "{page}"
        );
        assert!(!page.contains(r#"href="/logout""#), "{page}");
    }

    #[test]
    fn a_name_is_escaped() {
        let page =
            home_logged_in(&login(Some("<script>x</script>")), &me(&[7]), false).into_string();
        assert!(!page.contains("<script>"), "{page}");
    }

    #[test]
    fn the_planets_list_the_homeworld_first_then_ascending() {
        let page = home_logged_in(&login(None), &me(&[12, 7, 9]), false).into_string();
        let at = |s: &str| page.find(s).unwrap_or_else(|| panic!("{s}: {page}"));
        assert!(page.contains("Your planets (3)"), "{page}");
        assert!(at(r#"Planet #7</a> (homeworld)"#) < at(r#"Planet #9</a>"#));
        assert!(at(r#"Planet #9</a>"#) < at(r#"Planet #12</a>"#));
        assert!(!page.contains("Welcome."), "{page}");
        let first = home_logged_in(&login(None), &me(&[7]), false).into_string();
        assert!(
            first.contains("Welcome. Planet #7 is your homeworld"),
            "{first}"
        );
        assert!(first.contains("Your planets (1)"), "{first}");
    }
}
