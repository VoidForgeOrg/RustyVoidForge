//! The pages: plain HTML from maud, browser default styles, no script. P4.1 (D-057) brought the
//! login's pages; P4.2 (D-059) the frame every page shares (phase4-ux §2), Home's data (§4.1),
//! and the pages for a failed read: the retry page (§5.5) and the full universe (§4.1). P4.3
//! (D-060) brings the game's pages: Planet (§4.2), Fleets (§4.3), Universe (§4.4) and
//! Leaderboard (§4.5), with the navigation and the freshness line (§2) and the edge states of
//! §4.6. Every value is formatted by [`crate::format`]; nothing here formats a number.
//!
//! P4.4 (D-061) brings the forms that act (§5.3): each posts to `/act` with the page's form key,
//! its `return_to` and its `action`. The four irreversible actions are links to their confirm
//! pages (§5.4), which carry the real form. §4.3's lines that explain why an action isn't offered
//! stay where it's hidden, and §5.7's cost lines show when `/rules` answered.

use maud::{DOCTYPE, Markup, html};
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::rules::RulesView;
use voidforge_api_types::view::{
    BuildingView, FleetShip, FleetView, FleetsView, FlowStatus, Leaderboard, PlanetView,
    PlanetsNearView, PlayerView, ShipBuildState, ShipBuildView,
};
use voidforge_api_types::wire::{
    BuildingId, BuildingType, Cargo, Mission, PlayerId, Position, ShipType, Timestamp,
};

use crate::format::{self, Flow};
use crate::forms::{self, Posted};
use crate::messages::{self, Context, Fill, Message};
use crate::session::{Banner, LoggedIn};

/// A line shown above the logged-out Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The login expired, or the API stopped accepting it.
    Expired,
    /// The viewer just logged out (§4.1; P4.3's E9).
    LoggedOut,
    /// A form was submitted on an expired login, so it wasn't sent (D-057, carried to P4.4).
    ActionExpired,
    /// A form was submitted without a login, so it wasn't sent (P4.4).
    NotLoggedIn,
}

/// The advisor's sentence for an expired login (P4.1 A4; kept for P4.2 by its A1).
pub const EXPIRED: &str = "Your login has expired. Log in again.";
/// §5.6's one-shot line after logging in again from an ended session (carried by D-059).
pub const RENEWED: &str = "Your session had ended; you're logged in again.";
/// An operational Shipyard that no build names (P4.3 QA's F1): the server calls it `halted`
/// (D-025: no active builds), which isn't a shortage of ingots.
pub const IDLE_SHIPYARD: &str = "idle: no ships building";
/// §4.1's line after a logout.
pub const LOGGED_OUT: &str = "You're logged out.";
/// §4.6 (E8): a planet path that isn't a number, shown with `invalid_query`.
pub const NOT_A_PLANET_NUMBER: &str = "That isn't a planet number. Planet numbers look like 12.";
/// §6.3: a Universe search the web can't convert.
pub const UNIVERSE_INPUT: &str = "Enter numbers like 150 or −20.5; the radius must be 0 or more.";
/// §4.4: `invalid_query` from a search the web already checked means too many planets.
pub const TOO_MANY_PLANETS: &str =
    "Too many planets in that area (more than 1,000). Use a smaller radius.";
/// §6.1's note: `unavailable` on the leaderboard, before its first build.
pub const LEADERBOARD_NOT_READY: &str = "The leaderboard isn't ready yet. Try again in a moment.";

/// A page the navigation names (§2; P4.3's D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// `/`.
    Home,
    /// `/fleets`.
    Fleets,
    /// `/universe`.
    Universe,
    /// `/leaderboard`.
    Leaderboard,
}

const NAV: [(Nav, &str, &str); 4] = [
    (Nav::Home, "/", "Home"),
    (Nav::Fleets, "/fleets", "Fleets"),
    (Nav::Universe, "/universe", "Universe"),
    (Nav::Leaderboard, "/leaderboard", "Leaderboard"),
];

/// What every page's frame shows: the fast-rules line, and for a logged-in viewer who they are
/// and the navigation.
#[derive(Debug, Clone, Copy, Default)]
pub struct Frame<'a> {
    /// The rules are `v0-fast` (§2). False when `/rules` failed: the line is left out.
    pub fast: bool,
    /// The viewer, when logged in. The navigation shows only then (D3).
    pub login: Option<&'a LoggedIn>,
    /// The viewer's player, when `/players/me` answered.
    pub me: Option<&'a PlayerView>,
    /// The page the viewer is on, shown as plain text in the navigation; None links all four
    /// (a planet page, D3).
    pub nav: Option<Nav>,
    /// The one-shot banner at the top of the page (§2).
    pub banner: Option<&'a Banner>,
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
                    nav {
                        p {
                            strong { "Voidforge" } " "
                            @for (i, (page, href, label)) in NAV.iter().enumerate() {
                                @if i > 0 { " · " }
                                @if frame.nav == Some(*page) {
                                    (label)
                                } @else {
                                    a href=(href) { (label) }
                                }
                            }
                            @if let Some(me) = frame.me {
                                " · You: " (named(&me.player_id)) " · score " (format::thousands(me.score))
                            }
                        }
                    }
                    p { "Logged in as " strong { (who(login)) } "." }
                    form method="post" action="/logout" {
                        button type="submit" { "Log out" }
                    }
                    hr;
                    @if let Some(banner) = frame.banner {
                        p {
                            strong { (banner.text) }
                            @if let Some(c) = &banner.code { " (" code { (c) } ")" }
                        }
                        @for line in &banner.more {
                            p { (line) }
                        }
                    }
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

/// A player's short name, with the full ID in a hover (§3).
fn named(id: &PlayerId) -> Markup {
    html! { span title=(id.as_str()) { (format::short_name(id.as_str())) } }
}

/// A player as the viewer reads them: `you`, or the short name with the hover (§3).
fn player(id: &PlayerId, me: &PlayerId) -> Markup {
    html! {
        @if id == me { "you" } @else { (named(id)) }
    }
}

/// A planet's owner: `you`, a player, or `unowned` (§3).
fn owner(owner: Option<&PlayerId>, me: &PlayerId) -> Markup {
    html! {
        @match owner {
            Some(o) => (player(o, me)),
            None => "unowned",
        }
    }
}

fn planet_link(id: u64) -> Markup {
    html! { a href={ "/planets/" (id) } { "Planet #" (id) } }
}

/// The earliest of the times given, or None when every one is null (§2: several views).
fn earliest<'a>(times: impl IntoIterator<Item = Option<&'a Timestamp>>) -> Option<&'a Timestamp> {
    times
        .into_iter()
        .flatten()
        .min_by_key(|t| format::unix_ms(t).unwrap_or(i64::MIN))
}

/// §2's freshness line (copy, exact) and Refresh, a plain link to the same URL. `others` keeps
/// the clause about another player's fleet, with or without a `valid_until` (P4.3's design review
/// R2); the Fleets page drops it.
fn freshness(
    as_of: &Timestamp,
    valid_until: Option<&Timestamp>,
    others: bool,
    here: &str,
) -> Markup {
    html! {
        p {
            "As of " (format::clock(as_of, as_of)) ". "
            @match valid_until {
                Some(until) => {
                    "These values hold until " (format::clock(until, as_of))
                    @if others { ", unless another player's fleet arrives" }
                    "."
                }
                None => {
                    "Nothing is scheduled to change these values"
                    @if others { ", unless another player's fleet arrives" }
                    "."
                }
            }
            " " a href=(here) { "Refresh" }
        }
    }
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
            @match notice {
                Some(Notice::Expired) => p { strong { (EXPIRED) } },
                Some(Notice::LoggedOut) => p { strong { (LOGGED_OUT) } },
                Some(Notice::ActionExpired) => p { strong { (forms::ACTION_EXPIRED) } },
                Some(Notice::NotLoggedIn) => p { strong { (forms::NOT_LOGGED_IN) } },
                None => {}
            }
            p { "A persistent space strategy game. Log in to play." }
            p { a href=(login_href) { "Log in" } }
        },
    )
}

/// Home, logged in, with the player (§4.1).
#[must_use]
pub fn home_logged_in(
    login: &LoggedIn,
    me: &PlayerView,
    fast: bool,
    banner: Option<&Banner>,
) -> Markup {
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
            nav: Some(Nav::Home),
            banner,
        },
        html! {
            h1 { "Your empire" }
            @if owns_home && others.is_empty() {
                p { "Welcome. Planet #" (home) " is your homeworld: a Drill mines iron ore and a Refinery turns it into ingots. Build more, then a Shipyard to make ships." }
            }
            p { "Score: " (format::thousands(me.score)) " (current)" }
            p { "Score as of " (format::clock(&me.as_of, &me.as_of)) "." }
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

/// The flow words for an operational building of this type.
fn flow_of(t: BuildingType) -> Flow {
    match t {
        BuildingType::Drill => Flow::Drill,
        BuildingType::Refinery => Flow::Refinery,
        BuildingType::Shipyard => Flow::Shipyard,
    }
}

/// A construction's or a build's status after its progress, only when it isn't running: the
/// rate already says that (§4.6, E2).
fn receiving(status: Option<FlowStatus>) -> String {
    match status {
        Some(s @ (FlowStatus::Throttled | FlowStatus::Halted)) => {
            format!("; {}", format::flow(s, Flow::Receiving))
        }
        Some(FlowStatus::Running) | None => String::new(),
    }
}

/// A building's State cell (§4.2; §4.6's E1 and E2). An operational Shipyard with no builds is
/// idle, whatever its status: D-025 reports it `halted`, and §3's "halted: no ingots" would
/// blame its ingots (P4.3 QA's F1).
fn building_state(b: &BuildingView, p: &PlanetView) -> String {
    let as_of = &p.as_of;
    match b {
        BuildingView::Operational {
            id,
            building_type: BuildingType::Shipyard,
            ..
        } if !p.ship_builds.iter().any(|s| s.shipyard_id == *id) => IDLE_SHIPYARD.to_owned(),
        BuildingView::Operational {
            building_type,
            status,
            ..
        } => status
            .map_or("operational", |s| format::flow(s, flow_of(*building_type)))
            .to_owned(),
        BuildingView::UnderConstruction {
            status,
            delivered_g,
            cost_g,
            rate_g_per_h,
            eta,
            ..
        } => format!(
            "under construction: {}{}",
            format::progress(*delivered_g, *cost_g, *rate_g_per_h, eta.as_ref(), as_of),
            receiving(*status)
        ),
        BuildingView::Demolishing {
            demolition_ends_at, ..
        } => format!(
            "being demolished; the slot frees at {}",
            format::at(demolition_ends_at, as_of)
        ),
    }
}

fn building_parts(b: &BuildingView) -> (BuildingId, BuildingType) {
    match b {
        BuildingView::Operational {
            id, building_type, ..
        }
        | BuildingView::UnderConstruction {
            id, building_type, ..
        }
        | BuildingView::Demolishing {
            id, building_type, ..
        } => (*id, *building_type),
    }
}

/// The Shipyards that get a block (§4.2): each one operational or with builds, in view order,
/// then any build's Shipyard the buildings don't list, so no build is ever hidden.
fn shipyards(p: &PlanetView) -> Vec<BuildingId> {
    let mut ids: Vec<BuildingId> = p
        .buildings
        .iter()
        .filter(|b| {
            let (id, t) = building_parts(b);
            t == BuildingType::Shipyard
                && (matches!(b, BuildingView::Operational { .. })
                    || p.ship_builds.iter().any(|s| s.shipyard_id == id))
        })
        .map(|b| building_parts(b).0)
        .collect();
    for s in &p.ship_builds {
        if !ids.contains(&s.shipyard_id) {
            ids.push(s.shipyard_id);
        }
    }
    ids
}

/// One Shipyard's builds in view order, each with its State cell: `building: …` with progress,
/// or `waiting (Nth in line)`, numbering the waiting ones from 1 (§4.2).
fn yard_rows(p: &PlanetView, yard: BuildingId) -> Vec<(&ShipBuildView, String)> {
    let mut waiting = 0;
    p.ship_builds
        .iter()
        .filter(|s| s.shipyard_id == yard)
        .map(|s| {
            let state = match s.state {
                ShipBuildState::Active => format!(
                    "building: {}{}",
                    format::progress(
                        s.delivered_g,
                        s.cost_g,
                        s.rate_g_per_h,
                        s.eta.as_ref(),
                        &p.as_of
                    ),
                    receiving(s.status)
                ),
                ShipBuildState::Waiting => {
                    waiting += 1;
                    format!("waiting ({} in line)", format::ordinal(waiting))
                }
            };
            (s, state)
        })
        .collect()
}

/// The types in the order pages list them.
const SHIP_TYPES: [ShipType; 2] = [ShipType::ColonyShip, ShipType::CargoVessel];

/// Ships summarized by type: `1 Colony Ship, 2 Cargo Vessels`.
fn ship_summary(ships: &[FleetShip]) -> String {
    SHIP_TYPES
        .iter()
        .filter_map(|t| {
            let n = ships.iter().filter(|s| s.ship_type == *t).count();
            (n > 0).then(|| format::ships(n, *t))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A fleet's cargo, by resource, leaving out an empty one; None when there's none.
fn cargo_parts(c: &Cargo) -> Option<String> {
    let parts: Vec<String> = [(c.iron_ore_g, "ore"), (c.iron_ingot_g, "ingots")]
        .iter()
        .filter(|(g, _)| *g != 0)
        .map(|(g, what)| format!("{} of {what}", format::mass(*g)))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn has_cargo(c: &Cargo) -> bool {
    c.iron_ore_g != 0 || c.iron_ingot_g != 0
}

/// The API's name of a building type, as a form sends it.
fn wire_building(t: BuildingType) -> &'static str {
    match t {
        BuildingType::Drill => "drill",
        BuildingType::Refinery => "refinery",
        BuildingType::Shipyard => "shipyard",
    }
}

/// The API's name of a ship type, as a form sends it.
fn wire_ship(t: ShipType) -> &'static str {
    match t {
        ShipType::ColonyShip => "colony_ship",
        ShipType::CargoVessel => "cargo_vessel",
    }
}

/// The missions in the order forms offer them, with the API's names.
const MISSIONS: [(Mission, &str); 3] = [
    (Mission::Colonize, "colonize"),
    (Mission::Transport, "transport"),
    (Mission::Move, "move"),
];

/// A form that posts an order (§5.1): the page's key, where to return, the action, its hidden
/// fields, then what the player sets.
fn act_form(
    key: &str,
    return_to: &str,
    action: &str,
    hidden: &[(&str, String)],
    body: &Markup,
) -> Markup {
    html! {
        form method="post" action="/act" {
            input type="hidden" name="action" value=(action);
            input type="hidden" name="key" value=(key);
            input type="hidden" name="return_to" value=(return_to);
            @for (name, value) in hidden {
                input type="hidden" name=(name) value=(value);
            }
            (body)
        }
    }
}

/// A cost spread over a build: `150.0 t over 3 h (50.0 t/h)` (§5.7).
fn cost_over(cost_g: i64, build_time_ms: i64, what: &str) -> String {
    let rate = format::per_hour(cost_g, build_time_ms)
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    format!(
        "{}{what} over {}{rate}",
        format::mass(cost_g),
        format::duration(build_time_ms)
    )
}

/// A Build radio's words, with §5.7's line when `/rules` answered.
fn build_choice(t: BuildingType, rules: Option<&RulesView>) -> String {
    let name = format::building(t);
    let Some(r) = rules else {
        return name.to_owned();
    };
    let b = &r.buildings;
    match t {
        BuildingType::Drill => format!(
            "{name}: {} · mines {} t of ore per hour",
            cost_over(b.drill.cost_g, b.drill.build_time_ms, " of ingots"),
            format::tonnes(b.drill.output_g_per_h)
        ),
        BuildingType::Refinery => format!(
            "{name}: {} · turns {} t of ore into {} t of ingots per hour",
            cost_over(b.refinery.cost_g, b.refinery.build_time_ms, ""),
            format::tonnes(b.refinery.intake_g_per_h),
            format::tonnes(b.refinery.output_g_per_h)
        ),
        BuildingType::Shipyard => format!(
            "{name}: {} · builds up to {} ships at once",
            cost_over(b.shipyard.cost_g, b.shipyard.build_time_ms, ""),
            b.shipyard.parallel_builds
        ),
    }
}

/// A ship type's rules.
fn ship_rules(r: &RulesView, t: ShipType) -> &voidforge_api_types::rules::ShipKindRules {
    match t {
        ShipType::ColonyShip => &r.ships.colony_ship,
        ShipType::CargoVessel => &r.ships.cargo_vessel,
    }
}

/// What a ship carries: `carries 500.0 t`, or `carries nothing` (§5.7).
fn carries(capacity_g: i64) -> String {
    if capacity_g == 0 {
        "carries nothing".to_owned()
    } else {
        format!("carries {}", format::mass(capacity_g))
    }
}

/// A Queue radio's words: `Cargo Vessel: 300.0 t over 4 h · 15.0 u/h · carries 500.0 t` (§5.7).
fn ship_choice(t: ShipType, rules: Option<&RulesView>) -> String {
    let name = format::ship(t);
    match rules.map(|r| ship_rules(r, t)) {
        Some(s) => format!(
            "{name}: {} over {} · {} · {}",
            format::mass(s.cost_g),
            format::duration(s.build_time_ms),
            format::speed(s.speed_du_per_h),
            carries(s.cargo_capacity_g)
        ),
        None => name.to_owned(),
    }
}

/// A Buildings row's Action cell on your planet (§4.2): a link to the confirm page, the reason
/// Demolish isn't offered on a Shipyard with builds, or nothing while demolishing.
fn building_action(b: &BuildingView, p: &PlanetView) -> Markup {
    let planet = p.planet_id.0;
    html! {
        @match b {
            BuildingView::UnderConstruction { id, .. } => {
                a href={ "/planets/" (planet) "/cancel-construction/" (id.0) } { "Cancel construction…" }
            }
            BuildingView::Operational { id, building_type: BuildingType::Shipyard, .. }
                if p.ship_builds.iter().any(|s| s.shipyard_id == *id) => { "Cancel its ship builds first" }
            BuildingView::Operational { id, .. } => {
                a href={ "/planets/" (planet) "/demolish/" (id.0) } { "Demolish…" }
            }
            BuildingView::Demolishing { .. } => {}
        }
    }
}

/// Planet (§4.2). Owned by the viewer when the planet's owner is their player. The forms carry
/// `key` (§5.1); the cost lines come from `rules` when it answered (§5.7).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one page in §4.2's order reads best as one template"
)]
pub fn planet(
    frame: Frame<'_>,
    here: &str,
    me: &PlayerView,
    p: &PlanetView,
    fleets: &FleetsView,
    rules: Option<&RulesView>,
    key: &str,
) -> Markup {
    let id = p.planet_id.0;
    let you = &me.player_id;
    let mine = p.owner.as_ref() == Some(you);
    let as_of = &p.as_of;
    let blocks = shipyards(p);
    let full = p.buildings.len() >= usize::try_from(p.slots).unwrap_or(usize::MAX);
    // The server lists only the viewer's ships (D-049); "your ships" keeps to them regardless.
    let mut roster: Vec<_> = p.roster.iter().filter(|s| &s.owner == you).collect();
    roster.sort_by_key(|s| (SHIP_TYPES.iter().position(|t| *t == s.ship_type), s.ship_id));
    let mut here_fleets: Vec<_> = fleets
        .fleets
        .iter()
        .filter_map(|f| match f {
            FleetView::Stationed {
                fleet_id,
                ships,
                cargo,
                at_planet_id,
                ..
            } if at_planet_id.0 == id => Some((fleet_id.0, ships, cargo)),
            _ => None,
        })
        .collect();
    here_fleets.sort_by_key(|f| f.0);
    let mut elsewhere: Vec<_> = fleets
        .fleets
        .iter()
        .filter_map(|f| match f {
            FleetView::Stationed {
                fleet_id,
                ships,
                at_planet_id,
                ..
            } if at_planet_id.0 != id => Some((fleet_id.0, at_planet_id.0, ships)),
            _ => None,
        })
        .collect();
    elsewhere.sort_by_key(|f| f.0);
    let operational = |yard: &BuildingId| {
        p.buildings
            .iter()
            .any(|b| matches!(b, BuildingView::Operational { id, .. } if id == yard))
    };
    let stocks = &p.stocks;
    layout(
        &format!("Planet #{id}"),
        frame,
        html! {
            h1 { "Planet #" (id) }
            p {
                "Owner: " (owner(p.owner.as_ref(), you))
                @if mine && id == me.homeworld_id.0 { " (homeworld)" }
                " · Position " (format::position(&p.position))
                " · Slots: " (p.buildings.len()) " of " (p.slots) " used"
            }
            p { a href={ "/universe?around=" (id) } { "Look around this planet" } }
            (freshness(as_of, earliest([p.valid_until.as_ref(), fleets.valid_until.as_ref()]), true, here))

            h2 { "Storage" }
            table {
                thead { tr { th scope="col" { "Resource" } th scope="col" { "Stored" } th scope="col" { "Rate" } } }
                tbody {
                    @for (name, s) in [("Iron ore", &stocks.iron_ore), ("Iron ingots", &stocks.iron_ingot)] {
                        tr { th scope="row" { (name) } td { (format::stock(s)) } td { (format::rate(s.rate_g_per_h)) } }
                    }
                }
            }

            h2 { "Buildings" }
            @if p.buildings.is_empty() {
                p { "No buildings." }
            } @else {
                table {
                    thead {
                        tr {
                            th scope="col" { "Building" } th scope="col" { "State" }
                            @if mine { th scope="col" { "Action" } }
                        }
                    }
                    tbody {
                        @for b in &p.buildings {
                            @let (bid, t) = building_parts(b);
                            tr {
                                th scope="row" { (format::building(t)) " #" (bid.0) }
                                td { (building_state(b, p)) }
                                @if mine { td { (building_action(b, p)) } }
                            }
                        }
                    }
                }
            }
            @if mine {
                h3 { "Build" }
                (act_form(key, here, "build_building", &[("planet_id", id.to_string())], &html! {
                    @for t in [BuildingType::Drill, BuildingType::Refinery, BuildingType::Shipyard] {
                        p { label { input type="radio" name="building_type" value=(wire_building(t)); " " (build_choice(t, rules)) } }
                    }
                    @if full {
                        p { "Every slot is taken: demolish or cancel a building to free one." }
                        p { button type="submit" disabled { "Build" } }
                    } @else {
                        p { button type="submit" { "Build" } }
                    }
                }))
                p { "Ingots in storage: " (format::mass(stocks.iron_ingot.amount_g)) " (" (format::rate(stocks.iron_ingot.rate_g_per_h)) ")." }
                p { "A building takes a slot at once and draws ingots from storage until it's complete." }
            }

            @if !blocks.is_empty() {
                h2 { "Shipyards" }
                @for yard in &blocks {
                    @let rows = yard_rows(p, *yard);
                    h3 { "Shipyard #" (yard.0) }
                    @if rows.is_empty() {
                        p { "No ships building." }
                    } @else {
                        table {
                            thead {
                                tr {
                                    th scope="col" { "Build" } th scope="col" { "Ship" } th scope="col" { "State" }
                                    @if mine { th scope="col" { "Action" } }
                                }
                            }
                            tbody {
                                @for (s, state) in &rows {
                                    tr {
                                        th scope="row" { "build #" (s.id.0) }
                                        td { (format::ship(s.ship_type)) }
                                        td { (state) }
                                        @if mine {
                                            td { a href={ "/planets/" (id) "/cancel-build/" (s.id.0) } { "Cancel…" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    @if mine && operational(yard) {
                        (act_form(key, here, "queue_ship", &[("shipyard_id", yard.0.to_string())], &html! {
                            p { "Queue a ship:" }
                            @for t in SHIP_TYPES {
                                p { label { input type="radio" name="ship_type" value=(wire_ship(t)); " " (ship_choice(t, rules)) } }
                            }
                            p { button type="submit" { "Queue" } }
                        }))
                        @if let Some(r) = rules {
                            p { "Up to " (r.buildings.shipyard.parallel_builds) " ships build at once; the rest wait their turn." }
                        }
                    }
                }
            }

            @if !roster.is_empty() {
                h2 { "Your ships here" }
                (act_form(key, here, "assemble_fleet", &[("planet_id", id.to_string())], &html! {
                    @for s in &roster {
                        p {
                            label {
                                input type="checkbox" name="ship" value=(s.ship_id.0);
                                " " (format::ship(s.ship_type)) " #" (s.ship_id.0)
                                @if let Some(k) = rules.map(|r| ship_rules(r, s.ship_type)) {
                                    " (" (carries(k.cargo_capacity_g)) ", " (format::speed(k.speed_du_per_h)) ")"
                                }
                            }
                        }
                    }
                    p { "A fleet carries the total of its Cargo Vessels and flies at its slowest ship's speed." }
                    @if mine {
                        p {
                            "Cargo to load: "
                            label { "Iron ore " input type="text" name="ore" size="10"; " t" }
                            " (" (format::mass(stocks.iron_ore.amount_g)) " in storage) "
                            label { "Iron ingots " input type="text" name="ingots" size="10"; " t" }
                            " (" (format::mass(stocks.iron_ingot.amount_g)) " in storage)"
                        }
                    } @else {
                        p { "You can load cargo only at your own planets." }
                    }
                    p { button type="submit" { "Assemble fleet" } }
                }))
            }

            @if !here_fleets.is_empty() {
                h2 { "Your fleets here" }
                ul {
                    @for (fid, ships, cargo) in &here_fleets {
                        li {
                            "Fleet #" (fid) ": " (ship_summary(ships)) ", "
                            @match cargo_parts(cargo) {
                                Some(c) => { "carrying " (c) "." }
                                None => "carrying no cargo.",
                            }
                            " " a href={ "/fleets#fleet-" (fid) } { "Manage on the Fleets page" }
                        }
                    }
                }
            }

            @if !elsewhere.is_empty() {
                h2 { "Send a fleet here" }
                (act_form(key, here, "send_fleet", &[("target_planet_id", id.to_string())], &html! {
                    p {
                        label {
                            "Fleet: "
                            select name="fleet_id" {
                                option value="" { "Choose a fleet" }
                                @for (fid, at, ships) in &elsewhere {
                                    option value=(fid) { "Fleet #" (fid) " at Planet #" (at) " (" (ship_summary(ships)) ")" }
                                }
                            }
                        }
                    }
                    (mission_radios())
                    p { button type="submit" { "Send" } }
                }))
                p { "Colonize needs a Colony Ship and an unowned planet. Transport needs a Cargo Vessel and one of your planets. Move goes anywhere." }
            }
        },
    )
}

/// The three missions as radios, none preselected (§5.3).
fn mission_radios() -> Markup {
    html! {
        p {
            "Mission: "
            @for (m, wire) in MISSIONS {
                label { input type="radio" name="mission" value=(wire); " " (format::mission(m)) } " "
            }
        }
    }
}

/// The order §4.3 lists fleets in: stationed, in transit, returning; then by ID.
fn fleet_order(f: &FleetView) -> (u8, u64) {
    match f {
        FleetView::Stationed { fleet_id, .. } => (0, fleet_id.0),
        FleetView::InTransit { fleet_id, .. } => (1, fleet_id.0),
        FleetView::Returning { fleet_id, .. } => (2, fleet_id.0),
    }
}

/// Fleets (§4.3): the viewer's own, only ever (D-030). The forms carry `key`; `target`
/// pre-fills every Send form's target (§4.3's `?target=`).
#[must_use]
pub fn fleets(
    frame: Frame<'_>,
    here: &str,
    me: &PlayerView,
    view: &FleetsView,
    key: &str,
    target: Option<u64>,
) -> Markup {
    let acts = Acts { key, here, target };
    let as_of = &view.as_of;
    let mut list: Vec<&FleetView> = view.fleets.iter().collect();
    list.sort_by_key(|f| fleet_order(f));
    layout(
        "Your fleets",
        frame,
        html! {
            h1 { "Your fleets" }
            (freshness(as_of, view.valid_until.as_ref(), false, here))
            @if list.is_empty() {
                p { "You have no fleets. Assemble one from ships on a planet's roster." }
            }
            @for f in list {
                (fleet(f, me, as_of, &acts))
            }
        },
    )
}

/// What a Fleets page's forms share.
struct Acts<'a> {
    key: &'a str,
    here: &'a str,
    target: Option<u64>,
}

fn fleet(f: &FleetView, me: &PlayerView, as_of: &Timestamp, acts: &Acts<'_>) -> Markup {
    let (FleetView::Stationed {
        fleet_id,
        ships,
        cargo,
        capacity_g,
        speed_du_per_h,
        ..
    }
    | FleetView::InTransit {
        fleet_id,
        ships,
        cargo,
        capacity_g,
        speed_du_per_h,
        ..
    }
    | FleetView::Returning {
        fleet_id,
        ships,
        cargo,
        capacity_g,
        speed_du_per_h,
        ..
    }) = f;
    let fid = fleet_id.0;
    let mut ids: Vec<u64> = ships.iter().map(|s| s.ship_id.0).collect();
    ids.sort_unstable();
    let ids = ids
        .iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    // §8 allows a sum of a view's own amounts.
    let total = cargo.iron_ore_g.checked_add(cargo.iron_ingot_g);
    html! {
        section {
            h2 id={ "fleet-" (fid) } {
                "Fleet #" (fid) ": "
                @match f {
                    FleetView::Stationed { at_planet_id, .. } => { "stationed at " (planet_link(at_planet_id.0)) }
                    FleetView::InTransit { target_planet_id, mission, .. } => {
                        "flying to " (planet_link(target_planet_id.0)) " (" (format::mission(*mission)) ")"
                    }
                    FleetView::Returning { origin_planet_id, .. } => { "returning to " (planet_link(origin_planet_id.0)) }
                }
            }
            @match f {
                FleetView::InTransit { origin_planet_id, departed_at, arrives_at, .. } => {
                    p { "Left " (planet_link(origin_planet_id.0)) " at " (format::clock(departed_at, as_of)) "; arrives " (format::at(arrives_at, as_of)) }
                }
                FleetView::Returning { arrives_at, .. } => {
                    p { "Arrives back " (format::at(arrives_at, as_of)) }
                }
                FleetView::Stationed { .. } => {}
            }
            p { "Ships: " (ship_summary(ships)) " (" (ids) ")" }
            p {
                "Cargo: "
                @match (cargo_parts(cargo), total) {
                    (Some(c), Some(total)) => { (c) " (" (format::mass(total)) " of " (format::mass(*capacity_g)) " capacity)" }
                    (Some(c), None) => { (c) " (" (format::mass(*capacity_g)) " capacity)" }
                    (None, _) if *capacity_g > 0 => { "none (" (format::mass(*capacity_g)) " capacity)" }
                    (None, _) => "none",
                }
                " · Speed " (format::speed(*speed_du_per_h))
            }
            @let fid_field = [("fleet_id", fid.to_string())];
            @match f {
                FleetView::Stationed { at_planet_id, .. } => {
                    (act_form(acts.key, acts.here, "send_fleet", &fid_field, &html! {
                        p {
                            label {
                                "Send: Target planet # "
                                input type="text" name="target_planet_id" size="8" value=(acts.target.map(|t| t.to_string()).unwrap_or_default());
                            }
                        }
                        (mission_radios())
                        p { button type="submit" { "Send" } }
                    }))
                    @if has_cargo(cargo) {
                        @if me.planet_ids.contains(at_planet_id) {
                            (act_form(acts.key, acts.here, "trigger_unload", &fid_field, &html! {
                                p { button type="submit" { "Unload cargo" } }
                            }))
                        } @else {
                            p { "Move this fleet to one of your planets to unload its cargo." }
                        }
                        p { "Unload the cargo before disbanding." }
                    } @else {
                        (act_form(acts.key, acts.here, "disband_fleet", &[("fleet_id", fid.to_string()), ("planet_id", at_planet_id.0.to_string())], &html! {
                            p { button type="submit" { "Disband" } }
                        }))
                    }
                }
                FleetView::InTransit { .. } => {
                    p { a href={ "/fleets/" (fid) "/turn-back" } { "Turn back…" } }
                    p { "A fleet can turn back once; it takes as long to return as it has flown." }
                }
                FleetView::Returning { .. } => {
                    p { "Already turned back; it can't be recalled again." }
                }
            }
        }
    }
}

/// A Universe search as its form shows it: the fields' text, as typed or pre-filled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchForm {
    /// The centre's x, in world units.
    pub x: String,
    /// The centre's y.
    pub y: String,
    /// The centre's z.
    pub z: String,
    /// The radius.
    pub radius: String,
}

impl SearchForm {
    /// The form pre-filled from a centre and radius in distance units, with the ASCII `-` (D1).
    #[must_use]
    pub fn filled(centre: &Position, radius_du: i64) -> Self {
        Self {
            x: format::field_units(centre.x),
            y: format::field_units(centre.y),
            z: format::field_units(centre.z),
            radius: format::field_units(radius_du),
        }
    }
}

/// What a Universe page shows above its form.
#[derive(Debug, Clone, Copy)]
pub enum Found<'a> {
    /// The planets near a centre.
    Planets {
        /// The centre.
        centre: &'a Position,
        /// The planet it's around: the default's homeworld, or `?around=`.
        around: Option<u64>,
        /// The radius, in distance units.
        radius_du: i64,
        /// The view.
        near: &'a PlanetsNearView,
        /// The viewer.
        me: &'a PlayerView,
    },
    /// The search couldn't be made or answered: the copy, and the code when the API gave one.
    Refused(&'a str, Option<&'a str>),
}

/// Universe (§4.4): the search form, and the planets near its centre by distance, then ID.
#[must_use]
pub fn universe(frame: Frame<'_>, here: &str, form: &SearchForm, found: Found<'_>) -> Markup {
    layout(
        "Universe",
        frame,
        html! {
            h1 { "Universe" }
            @match found {
                Found::Planets { centre, around, radius_du, near, .. } => {
                    p {
                        "Planets within " (format::distance(radius_du)) " of " (format::position(centre))
                        @if let Some(a) = around { ", around " (planet_link(a)) }
                        "."
                    }
                    (freshness(&near.as_of, near.valid_until.as_ref(), true, here))
                }
                Found::Refused(copy, code) => {
                    p { strong { (copy) } @if let Some(c) = code { " (" code { (c) } ")" } }
                }
            }
            form method="get" action="/universe" {
                p {
                    "Centre: "
                    label { "x " input type="text" name="x" value=(form.x) size="10"; } " "
                    label { "y " input type="text" name="y" value=(form.y) size="10"; } " "
                    label { "z " input type="text" name="z" value=(form.z) size="10"; } " "
                    label { "Radius " input type="text" name="radius" value=(form.radius) size="8"; " u" } " "
                    button type="submit" { "Search" }
                }
            }
            @if let Found::Planets { radius_du, near, me, .. } = found {
                (near_table(me, near, radius_du))
            }
        },
    )
}

fn near_table(me: &PlayerView, near: &PlanetsNearView, radius_du: i64) -> Markup {
    let mut planets: Vec<_> = near.planets.iter().collect();
    planets.sort_by_key(|p| (p.distance, p.planet_id));
    html! {
        @if planets.is_empty() {
            p { "No planets within " (format::distance(radius_du)) " of that point." }
        } @else {
            table {
                thead { tr { th scope="col" { "Planet" } th scope="col" { "Owner" } th scope="col" { "Distance" } th scope="col" { "Position" } } }
                tbody {
                    @for p in planets {
                        tr {
                            th scope="row" { (planet_link(p.planet_id.0)) }
                            td {
                                (owner(p.owner.as_ref(), &me.player_id))
                                @if p.owner.as_ref() == Some(&me.player_id) && p.planet_id == me.homeworld_id { " (homeworld)" }
                            }
                            td { (format::distance(p.distance)) }
                            td { (format::position(&p.position)) }
                        }
                    }
                }
            }
        }
    }
}

/// Leaderboard (§4.5): ranks as given (ties share one, D-051), the viewer's row in `strong`.
#[must_use]
pub fn leaderboard(frame: Frame<'_>, here: &str, me: &PlayerView, board: &Leaderboard) -> Markup {
    let you = &me.player_id;
    let mine = board.entries.iter().find(|e| &e.player_id == you);
    layout(
        "Leaderboard",
        frame,
        html! {
            h1 { "Leaderboard" }
            p {
                "Ranked as of " (format::clock(&board.as_of, &board.as_of))
                ". The board is rebuilt every minute; your score in the header is current. "
                a href=(here) { "Refresh" }
            }
            p {
                @match mine {
                    Some(e) => { "You're ranked " (e.rank) " of " (board.entries.len()) "." }
                    None => "You're not on the board yet; it's rebuilt every minute.",
                }
            }
            table {
                thead { tr { th scope="col" { "Rank" } th scope="col" { "Player" } th scope="col" { "Score" } } }
                tbody {
                    @for e in &board.entries {
                        tr {
                            @if &e.player_id == you {
                                td { strong { (e.rank) } } td { strong { "you" } } td { strong { (format::thousands(e.score)) } }
                            } @else {
                                td { (e.rank) } td { (named(&e.player_id)) } td { (format::thousands(e.score)) }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// §4.2: no such planet, from the API's `unknown_planet` or the web's own check of the path
/// (E8), with a link to the Universe page.
#[must_use]
pub fn no_such_planet(frame: Frame<'_>, m: &Message) -> Markup {
    layout(
        "No such planet",
        frame,
        html! {
            h1 { "No such planet" }
            p { (m.copy) (code(m)) }
            p { a href="/universe" { "Find a planet on the Universe page" } }
        },
    )
}

/// §6.2: a web page that doesn't exist.
#[must_use]
pub fn not_found() -> Markup {
    layout(
        "Not found",
        Frame::default(),
        html! {
            h1 { "Not found" }
            p { "There's no page here. " a href="/" { "Home" } }
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

/// §5.5 for a submitted order: the copy and code, **Try again** posting the identical fields
/// with the same key, and a link back to `return_to` that sends nothing (P4.4's D1 labels it).
#[must_use]
pub fn command_retry(frame: Frame<'_>, m: &Message, posted: &Posted, return_to: &str) -> Markup {
    layout(
        "Not sent yet",
        frame,
        html! {
            h1 { "Not sent yet" }
            p { (m.copy) (code(m)) }
            form method="post" action="/act" {
                @for (name, value) in &posted.pairs {
                    input type="hidden" name=(name) value=(value);
                }
                button type="submit" { "Try again" }
            }
            p { a href=(return_to) { (forms::back_label(return_to)) } }
        },
    )
}

/// What a confirm page offers (§5.4): the real form, or why there's nothing left to confirm.
enum Offer {
    /// The consequence line, the action, its hidden fields and the button's words.
    Form {
        line: String,
        action: &'static str,
        hidden: Vec<(&'static str, String)>,
        button: String,
    },
    /// The thing is gone or changed since the page it came from: the matching code's copy.
    Gone(ErrorCode),
}

/// A confirm page (§5.4): the question, then the consequence line and the real form with this
/// page's own key, and `Keep it` back to the page it came from; or the code's copy and a link
/// back, with no form.
fn confirm(
    frame: Frame<'_>,
    question: &str,
    offer: Offer,
    key: &str,
    back: &str,
    fill: Fill,
) -> Markup {
    layout(
        question,
        frame,
        html! {
            h1 { (question) }
            @match offer {
                Offer::Form { line, action, hidden, button } => {
                    p { (line) }
                    (act_form(key, back, action, &hidden, &html! {
                        p { button type="submit" { (button) } }
                    }))
                    p { a href=(back) { "Keep it" } }
                }
                Offer::Gone(code) => {
                    @let m = Message {
                        copy: messages::copy(code, Context::Command, fill),
                        code: Some(code.as_str()),
                        kind: messages::Kind::Banner,
                    };
                    p { (m.copy) (self::code(&m)) }
                    p { a href=(back) { (forms::back_label(back)) } }
                }
            }
        },
    )
}

/// The building `id` on `p`, if the view lists it.
fn find_building(p: &PlanetView, id: u64) -> Option<&BuildingView> {
    p.buildings.iter().find(|b| building_parts(b).0.0 == id)
}

/// Whether the viewer owns `p`.
fn owns(p: &PlanetView, me: &PlayerView) -> bool {
    p.owner.as_ref() == Some(&me.player_id)
}

/// §5.4: cancel construction of building `b` on `p`, losing the ingots delivered so far (as of
/// this page's own read).
#[must_use]
pub fn confirm_cancel_construction(
    frame: Frame<'_>,
    me: &PlayerView,
    p: &PlanetView,
    b: u64,
    key: &str,
) -> Markup {
    let planet = p.planet_id.0;
    let back = format!("/planets/{planet}");
    let fill = Fill {
        planet: Some(planet),
        target: None,
    };
    let found = find_building(p, b);
    let name = found.map_or_else(
        || format!("building #{b}"),
        |x| format!("{} #{b}", format::building(building_parts(x).1)),
    );
    let offer = match found {
        None => Offer::Gone(ErrorCode::UnknownBuilding),
        Some(_) if !owns(p, me) => Offer::Gone(ErrorCode::NotOwner),
        Some(BuildingView::UnderConstruction {
            building_type,
            delivered_g,
            ..
        }) => Offer::Form {
            line: format!(
                "Planet #{planet}. The {} of ingots delivered so far are lost, and the slot frees now.",
                format::mass(*delivered_g)
            ),
            action: "cancel_construction",
            hidden: vec![
                ("building_id", b.to_string()),
                ("building_type", wire_building(*building_type).to_owned()),
            ],
            button: format!("Cancel construction of {name}"),
        },
        Some(_) => Offer::Gone(ErrorCode::NotUnderConstruction),
    };
    confirm(
        frame,
        &format!("Cancel construction of {name}?"),
        offer,
        key,
        &back,
        fill,
    )
}

/// The demolition time of a building type (§5.7).
fn demolition_ms(r: &RulesView, t: BuildingType) -> i64 {
    match t {
        BuildingType::Drill => r.buildings.drill.demolition_time_ms,
        BuildingType::Refinery => r.buildings.refinery.demolition_time_ms,
        BuildingType::Shipyard => r.buildings.shipyard.demolition_time_ms,
    }
}

/// §5.4: demolish building `b` on `p`, with nothing refunded; the time the slot frees comes from
/// `/rules` when it answered (§5.7).
#[must_use]
pub fn confirm_demolish(
    frame: Frame<'_>,
    me: &PlayerView,
    p: &PlanetView,
    rules: Option<&RulesView>,
    b: u64,
    key: &str,
) -> Markup {
    let planet = p.planet_id.0;
    let back = format!("/planets/{planet}");
    let fill = Fill {
        planet: Some(planet),
        target: None,
    };
    let found = find_building(p, b);
    let name = found.map_or_else(
        || format!("building #{b}"),
        |x| format!("{} #{b}", format::building(building_parts(x).1)),
    );
    let offer = match found {
        None => Offer::Gone(ErrorCode::UnknownBuilding),
        Some(_) if !owns(p, me) => Offer::Gone(ErrorCode::NotOwner),
        Some(BuildingView::Operational {
            id,
            building_type: BuildingType::Shipyard,
            ..
        }) if p.ship_builds.iter().any(|s| s.shipyard_id == *id) => {
            Offer::Gone(ErrorCode::ShipyardHasBuilds)
        }
        Some(BuildingView::Operational { building_type, .. }) => Offer::Form {
            line: match rules {
                Some(r) => format!(
                    "Planet #{planet}. It stops working now, and nothing is refunded. The slot frees in {}, when demolition ends.",
                    format::duration(demolition_ms(r, *building_type))
                ),
                None => format!(
                    "Planet #{planet}. It stops working now, and nothing is refunded. The slot frees when demolition ends."
                ),
            },
            action: "demolish_building",
            hidden: vec![
                ("building_id", b.to_string()),
                ("building_type", wire_building(*building_type).to_owned()),
            ],
            button: format!("Demolish {name}"),
        },
        Some(_) => Offer::Gone(ErrorCode::NotOperational),
    };
    confirm(frame, &format!("Demolish {name}?"), offer, key, &back, fill)
}

/// §5.4: cancel ship build `n` on `p`, losing the ingots delivered so far.
#[must_use]
pub fn confirm_cancel_build(
    frame: Frame<'_>,
    me: &PlayerView,
    p: &PlanetView,
    n: u64,
    key: &str,
) -> Markup {
    let planet = p.planet_id.0;
    let back = format!("/planets/{planet}");
    let fill = Fill {
        planet: Some(planet),
        target: None,
    };
    let found = p.ship_builds.iter().find(|s| s.id.0 == n);
    let question = found.map_or_else(
        || format!("Cancel build #{n}?"),
        |s| {
            format!(
                "Cancel build #{n} ({}) at Shipyard #{}?",
                format::ship(s.ship_type),
                s.shipyard_id.0
            )
        },
    );
    let offer = match found {
        None => Offer::Gone(ErrorCode::UnknownShipBuild),
        Some(_) if !owns(p, me) => Offer::Gone(ErrorCode::NotOwner),
        Some(s) => Offer::Form {
            line: format!(
                "Planet #{planet}. The {} of ingots delivered so far are lost.",
                format::mass(s.delivered_g)
            ),
            action: "cancel_ship_build",
            hidden: vec![("ship_build_id", n.to_string())],
            button: format!("Cancel build #{n}"),
        },
    };
    confirm(frame, &question, offer, key, &back, fill)
}

/// How long a fleet has flown as the page shows it: `as_of` and `departed_at` each as their
/// clocks show them (both at or before `as_of`, so cut to the second), then the difference
/// (dev-design's R1 rule: a derived value comes from the shown ones).
fn flown(departed_at: &Timestamp, as_of: &Timestamp) -> String {
    let secs = match format::unix_ms(departed_at).zip(format::unix_ms(as_of)) {
        Some((d, a)) => (a.div_euclid(1_000) - d.div_euclid(1_000)).max(0),
        None => 0,
    };
    format::duration(secs.saturating_mul(1_000))
}

/// §5.4: turn fleet `f` back, once, abandoning its mission. The time flown is from this page's
/// own read of the fleets.
#[must_use]
pub fn confirm_turn_back(frame: Frame<'_>, view: &FleetsView, f: u64, key: &str) -> Markup {
    let back = "/fleets";
    let found = view.fleets.iter().find(|x| fleet_order(x).1 == f);
    let offer = match found {
        None => Offer::Gone(ErrorCode::UnknownFleet),
        Some(FleetView::Stationed { .. }) => Offer::Gone(ErrorCode::FleetNotInTransit),
        Some(FleetView::Returning { .. }) => Offer::Gone(ErrorCode::AlreadyRecalled),
        Some(FleetView::InTransit {
            origin_planet_id,
            target_planet_id,
            mission,
            departed_at,
            ..
        }) => Offer::Form {
            line: format!(
                "It has flown {}, so it returns to Planet #{} in about that long, and can't be turned again. Its mission ({} Planet #{}) is abandoned.",
                flown(departed_at, &view.as_of),
                origin_planet_id.0,
                format::mission(*mission),
                target_planet_id.0
            ),
            action: "recall_fleet",
            hidden: vec![
                ("fleet_id", f.to_string()),
                ("origin", origin_planet_id.0.to_string()),
            ],
            button: format!("Turn Fleet #{f} back"),
        },
    };
    confirm(
        frame,
        &format!("Turn Fleet #{f} back?"),
        offer,
        key,
        back,
        Fill::default(),
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
    fn logout_is_a_post_form() {
        let page = home_logged_in(&login(None), &me(&[7]), false, None).into_string();
        assert!(
            page.contains(r#"<form method="post" action="/logout">"#),
            "{page}"
        );
        assert!(!page.contains(r#"href="/logout""#), "{page}");
    }

    #[test]
    fn a_name_is_escaped() {
        let page = home_logged_in(&login(Some("<script>x</script>")), &me(&[7]), false, None)
            .into_string();
        assert!(!page.contains("<script>"), "{page}");
    }

    #[test]
    fn the_planets_list_the_homeworld_first_then_ascending() {
        let page = home_logged_in(&login(None), &me(&[12, 7, 9]), false, None).into_string();
        let at = |s: &str| page.find(s).unwrap_or_else(|| panic!("{s}: {page}"));
        assert!(page.contains("Your planets (3)"), "{page}");
        assert!(at(r#"Planet #7</a> (homeworld)"#) < at(r#"Planet #9</a>"#));
        assert!(at(r#"Planet #9</a>"#) < at(r#"Planet #12</a>"#));
        assert!(!page.contains("Welcome."), "{page}");
        let first = home_logged_in(&login(None), &me(&[7]), false, None).into_string();
        assert!(
            first.contains("Welcome. Planet #7 is your homeworld"),
            "{first}"
        );
        assert!(first.contains("Your planets (1)"), "{first}");
    }

    /// D3: the current page is plain text, the others links; no navigation logged out.
    #[test]
    fn the_navigation_marks_the_current_page() {
        let page = home_logged_in(&login(None), &me(&[7]), false, None).into_string();
        assert!(
            page.contains(r#"<strong>Voidforge</strong> Home · <a href="/fleets">Fleets</a> · <a href="/universe">Universe</a> · <a href="/leaderboard">Leaderboard</a> · You:"#),
            "{page}"
        );
        let out = home_logged_out(None, "/login", false).into_string();
        assert!(
            !out.contains("<nav>") && !out.contains(r#"href="/fleets""#),
            "{out}"
        );
    }
}
