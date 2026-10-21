//! The pages: plain HTML from maud, browser default styles, no script. P4.1 (D-057) brought the
//! login's pages; P4.2 (D-059) the frame every page shares (phase4-ux §2), Home's data (§4.1),
//! and the pages for a failed read: the retry page (§5.5) and the full universe (§4.1). P4.3
//! (D-060) brings the game's pages: Planet (§4.2), Fleets (§4.3), Universe (§4.4) and
//! Leaderboard (§4.5), with the navigation and the freshness line (§2) and the edge states of
//! §4.6. Every value is formatted by [`crate::format`]; nothing here formats a number.
//!
//! The forms that act are P4.4's: these pages render no form control and no placeholder for
//! them (T2, D2), only the information around them, including §4.3's lines that explain why an
//! action isn't offered.

use maud::{DOCTYPE, Markup, html};
use voidforge_api_types::view::{
    BuildingView, FleetShip, FleetView, FleetsView, FlowStatus, Leaderboard, PlanetView,
    PlanetsNearView, PlayerView, ShipBuildState, ShipBuildView,
};
use voidforge_api_types::wire::{
    BuildingId, BuildingType, Cargo, PlayerId, Position, ShipType, Timestamp,
};

use crate::format::{self, Flow};
use crate::messages::Message;
use crate::session::LoggedIn;

/// A line shown above the logged-out Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The login expired, or the API stopped accepting it.
    Expired,
    /// The viewer just logged out (§4.1; P4.3's E9).
    LoggedOut,
}

/// The advisor's sentence for an expired login (P4.1 A4; kept for P4.2 by its A1).
pub const EXPIRED: &str = "Your login has expired. Log in again.";
/// §5.6's one-shot line after logging in again from an ended session (carried by D-059).
pub const RENEWED: &str = "Your session had ended; you're logged in again.";
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
    pub banner: Option<&'a str>,
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
                        p { strong { (banner) } }
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
/// the clause about another player's fleet, which the Fleets page drops.
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
                None => "Nothing is scheduled to change these values.",
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
    banner: Option<&str>,
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

/// A building's State cell (§4.2; §4.6's E1 and E2).
fn building_state(b: &BuildingView, as_of: &Timestamp) -> String {
    match b {
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

/// Planet (§4.2). Owned by the viewer when the planet's owner is their player.
#[must_use]
pub fn planet(
    frame: Frame<'_>,
    here: &str,
    me: &PlayerView,
    p: &PlanetView,
    fleets: &FleetsView,
) -> Markup {
    let id = p.planet_id.0;
    let you = &me.player_id;
    let mine = p.owner.as_ref() == Some(you);
    let as_of = &p.as_of;
    let blocks = shipyards(p);
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
                    @for (name, s) in [("Iron ore", &p.stocks.iron_ore), ("Iron ingots", &p.stocks.iron_ingot)] {
                        tr { th scope="row" { (name) } td { (format::stock(s)) } td { (format::rate(s.rate_g_per_h)) } }
                    }
                }
            }

            h2 { "Buildings" }
            @if p.buildings.is_empty() {
                p { "No buildings." }
            } @else {
                table {
                    thead { tr { th scope="col" { "Building" } th scope="col" { "State" } } }
                    tbody {
                        @for b in &p.buildings {
                            @let (bid, t) = building_parts(b);
                            tr { th scope="row" { (format::building(t)) " #" (bid.0) } td { (building_state(b, as_of)) } }
                        }
                    }
                }
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
                            thead { tr { th scope="col" { "Build" } th scope="col" { "Ship" } th scope="col" { "State" } } }
                            tbody {
                                @for (s, state) in &rows {
                                    tr {
                                        th scope="row" { "build #" (s.id.0) }
                                        td { (format::ship(s.ship_type)) }
                                        td { (state) }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            @if !roster.is_empty() {
                h2 { "Your ships here" }
                ul {
                    @for s in &roster {
                        li { (format::ship(s.ship_type)) " #" (s.ship_id.0) }
                    }
                }
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
        },
    )
}

/// The order §4.3 lists fleets in: stationed, in transit, returning; then by ID.
fn fleet_order(f: &FleetView) -> (u8, u64) {
    match f {
        FleetView::Stationed { fleet_id, .. } => (0, fleet_id.0),
        FleetView::InTransit { fleet_id, .. } => (1, fleet_id.0),
        FleetView::Returning { fleet_id, .. } => (2, fleet_id.0),
    }
}

/// Fleets (§4.3): the viewer's own, only ever (D-030).
#[must_use]
pub fn fleets(frame: Frame<'_>, here: &str, me: &PlayerView, view: &FleetsView) -> Markup {
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
                (fleet(f, me, as_of))
            }
        },
    )
}

fn fleet(f: &FleetView, me: &PlayerView, as_of: &Timestamp) -> Markup {
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
            @match f {
                FleetView::Stationed { at_planet_id, .. } if has_cargo(cargo) => {
                    @if !me.planet_ids.contains(at_planet_id) {
                        p { "Move this fleet to one of your planets to unload its cargo." }
                    }
                    p { "Unload the cargo before disbanding." }
                }
                FleetView::Returning { .. } => {
                    p { "Already turned back; it can't be recalled again." }
                }
                _ => {}
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
