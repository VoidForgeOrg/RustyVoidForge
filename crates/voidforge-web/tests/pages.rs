//! The game's pages by effect through the router against the API stand-in (P4.3, D-060): Planet
//! (phase4-ux §4.2), Fleets (§4.3), Universe (§4.4) and Leaderboard (§4.5), their edge states
//! (§4.6) and their errors (§4.2, §4.4, §5.5, §6).
//!
//! Each page has an HTML snapshot (`tests/snapshots/`), and **beside every snapshot a by-effect
//! assertion** of what it must show and which reads it made (T4), so a wrong first render can't
//! be blessed by the snapshot alone. Snapshots are broken before block tags to read as diffs.

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use common::{Browser, PLAYER, Reply, World, player_view, rules_view};
use serde_json::{Value, json};

/// Another player.
const OTHER: &str = "01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f";
const AS_OF: &str = "2026-10-20T18:04:12.000Z";

async fn logged_in(w: &World) -> Browser {
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    b
}

/// The tags a snapshot breaks its line before.
const BREAK_BEFORE: [&str; 21] = [
    "<head",
    "<title",
    "<body",
    "<nav",
    "<p>",
    "<p ",
    "<form",
    "<hr",
    "<h1",
    "<h2",
    "<h3",
    "<table",
    "<thead",
    "<tbody",
    "<tr",
    "</table",
    "<ul",
    "<li",
    "</ul",
    "<section",
    "</section",
];

fn pretty(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + html.len() / 16);
    for (i, c) in html.char_indices() {
        if i > 0 && BREAK_BEFORE.iter().any(|t| html[i..].starts_with(t)) {
            out.push('\n');
        }
        out.push(c);
    }
    out
}

/// The requests made since `from`, as (path and query, authenticated), sorted: concurrent reads
/// arrive in any order.
fn reads_since(w: &World, from: usize) -> Vec<(String, bool)> {
    let mut r: Vec<_> = w.api.requests.lock().unwrap()[from..]
        .iter()
        .map(|r| (r.uri.clone(), r.authorization.is_some()))
        .collect();
    r.sort();
    r
}

fn count(w: &World) -> usize {
    w.api.requests.lock().unwrap().len()
}

fn reads(paths: &[&str]) -> Vec<(String, bool)> {
    let mut r: Vec<_> = paths
        .iter()
        .map(|p| ((*p).to_owned(), *p != "/rules"))
        .collect();
    r.sort();
    r
}

fn has(body: &str, want: &str) {
    assert!(
        body.contains(want),
        "missing {want:?} in:\n{}",
        pretty(body)
    );
}

fn lacks(body: &str, unwanted: &str) {
    assert!(
        !body.contains(unwanted),
        "unexpected {unwanted:?} in:\n{}",
        pretty(body)
    );
}

/// T2: no form control but Log out's one button (P4.4 brings the forms).
fn no_controls(body: &str) {
    assert_eq!(body.matches("<form").count(), 1, "{}", pretty(body));
    assert_eq!(body.matches("<button").count(), 1, "{}", pretty(body));
    for control in ["<input", "<select", "<textarea"] {
        lacks(body, control);
    }
}

fn reply(w: &World, path: &str, body: &Value) {
    w.api.reply(path, Reply::json(StatusCode::OK, body));
}

fn stock(amount_g: i64, rate_g_per_h: i64, cap_g: i64) -> Value {
    json!({"amount_g": amount_g, "rate_g_per_h": rate_g_per_h, "cap_g": cap_g})
}

/// The server's own route snapshot of a homeworld (`planet_view_alice_home.json`), as planet 7,
/// owned by `owner`.
fn planet_home(owner: Option<&str>) -> Value {
    let mut p: Value =
        serde_json::from_str(include_str!("fixtures/planet_view_alice_home.json")).unwrap();
    p["planet_id"] = json!(7);
    p["owner"] = json!(owner);
    p
}

fn planet(id: u64, owner: Option<&str>, buildings: Value, builds: Value, roster: Value) -> Value {
    json!({
        "planet_id": id, "owner": owner, "as_of": AS_OF,
        "valid_until": "2026-10-20T18:04:15.000Z",
        "stocks": {
            "iron_ore": stock(1_234_567_890, 2_000_000, 10_000_000_000_i64),
            "iron_ingot": stock(5_000_000_000_i64, 0, 5_000_000_000_i64),
        },
        "buildings": buildings, "position": {"x": 150_000, "y": -20_500, "z": 999_999},
        "slots": 8, "ship_builds": builds, "roster": roster,
    })
}

fn fleets_view(fleets: Value, valid_until: Option<&str>) -> Value {
    json!({"as_of": AS_OF, "valid_until": valid_until, "fleets": fleets})
}

fn stationed(id: u64, at: u64, ships: Value, ore: i64, ingots: i64, capacity: i64) -> Value {
    json!({
        "state": "stationed", "fleet_id": id, "ships": ships,
        "cargo": {"iron_ore_g": ore, "iron_ingot_g": ingots},
        "capacity_g": capacity, "speed_du_per_h": 10_000, "at_planet_id": at,
    })
}

fn ships(list: &[(u64, &str)]) -> Value {
    Value::Array(
        list.iter()
            .map(|(id, t)| json!({"ship_id": id, "ship_type": t}))
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------------
// Planet (§4.2)

#[tokio::test]
async fn your_homeworld_from_the_server_s_own_view() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    reply(&w, "/planets/7", &planet_home(Some(PLAYER)));
    let here = stationed(
        21,
        7,
        ships(&[(31, "cargo_vessel"), (30, "cargo_vessel")]),
        0,
        500_000_000,
        1_000_000_000,
    );
    let away = stationed(22, 9, ships(&[(40, "colony_ship")]), 0, 0, 0);
    reply(
        &w,
        "/players/me/fleets",
        &fleets_view(json!([away, here]), Some("2025-10-09T09:00:00.000Z")),
    );
    let from = count(&w);
    let page = b.get("/planets/7").await;
    assert_eq!(page.status, 200, "{}", page.body);
    // All four reads, once each; `/rules` without the token (T1).
    assert_eq!(
        reads_since(&w, from),
        reads(&["/planets/7", "/players/me", "/players/me/fleets", "/rules"])
    );
    let body = &page.body;
    has(body, "<h1>Planet #7</h1>");
    has(
        body,
        "Owner: you (homeworld) · Position (0.0, 0.0, 0.0) · Slots: 3 of 8 used",
    );
    has(
        body,
        r#"<a href="/universe?around=7">Look around this planet</a>"#,
    );
    // The earliest valid_until of the two views shown (§2): the fleets', not the planet's.
    has(
        body,
        "As of 08:53:20 UTC. These values hold until 09:00:00 UTC, unless another player's fleet arrives. <a href=\"/planets/7\">Refresh</a>",
    );
    has(
        body,
        r#"<th scope="row">Iron ore</th><td>2,000.0 t of 10,000.0 t</td><td>+2.0 t/h</td>"#,
    );
    has(
        body,
        r#"<th scope="row">Iron ingots</th><td>1,500.0 t of 5,000.0 t</td><td>−34.0 t/h</td>"#,
    );
    has(body, r#"<th scope="row">Drill #2</th><td>running</td>"#);
    has(body, r#"<th scope="row">Refinery #3</th><td>running</td>"#);
    has(
        body,
        r#"<th scope="row">Drill #8</th><td>under construction: 0% (0 of 150.0 t), +50.0 t/h, done at 11:53:20 UTC (in 3 h)</td>"#,
    );
    has(
        body,
        r##"Fleet #21: 2 Cargo Vessels, carrying 500.0 t of ingots. <a href="/fleets#fleet-21">Manage on the Fleets page</a>"##,
    );
    lacks(body, "Fleet #22");
    lacks(body, "<h2>Shipyards</h2>");
    lacks(body, "<h2>Your ships here</h2>");
    // D3: on a planet page all four are links.
    has(
        body,
        r#"<a href="/">Home</a> · <a href="/fleets">Fleets</a> · <a href="/universe">Universe</a> · <a href="/leaderboard">Leaderboard</a>"#,
    );
    no_controls(body);
    insta::assert_snapshot!("planet_owner", pretty(body));
}

#[tokio::test]
async fn every_building_and_build_state() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let eta = "2026-10-20T18:10:00.000Z";
    let buildings = json!([
        // First in the view, so its block is placed by the view's order, not appended after.
        {"state": "demolishing", "id": 19, "type": "shipyard", "demolition_ends_at": "2026-10-21T06:00:00.000Z"},
        {"state": "operational", "id": 2, "type": "drill", "status": "running"},
        {"state": "operational", "id": 3, "type": "refinery", "status": "throttled"},
        {"state": "operational", "id": 4, "type": "drill", "status": "halted"},
        {"state": "operational", "id": 5, "type": "refinery", "status": "halted"},
        {"state": "operational", "id": 15, "type": "shipyard", "status": "running"},
        {"state": "operational", "id": 16, "type": "shipyard"},
        {"state": "under_construction", "id": 17, "type": "shipyard", "status": "throttled",
         "delivered_g": 504_000_000, "cost_g": 1_200_000_000, "rate_g_per_h": 100_000_000, "eta": eta},
        {"state": "under_construction", "id": 18, "type": "refinery", "status": "halted",
         "delivered_g": 1, "cost_g": 250_000_000, "rate_g_per_h": 0, "eta": null},
        {"state": "demolishing", "id": 12, "type": "drill", "demolition_ends_at": "2026-10-20T18:05:00.000Z"},
    ]);
    let build = |id: u64,
                 yard: u64,
                 t: &str,
                 state: &str,
                 status: Option<&str>,
                 eta: Option<&str>| {
        let mut v = json!({
            "id": id, "shipyard_id": yard, "ship_type": t, "state": state,
            "delivered_g": 240_000_000, "cost_g": 800_000_000, "rate_g_per_h": 80_000_000, "eta": eta,
        });
        if let Some(s) = status {
            v["status"] = json!(s);
        }
        v
    };
    let builds = json!([
        build(20, 15, "colony_ship", "active", Some("running"), Some(eta)),
        build(21, 15, "cargo_vessel", "waiting", None, None),
        build(
            22,
            15,
            "cargo_vessel",
            "active",
            Some("throttled"),
            Some(eta)
        ),
        build(23, 15, "colony_ship", "waiting", None, None),
        // A demolishing Shipyard keeps its block while it has builds.
        build(24, 19, "cargo_vessel", "active", Some("halted"), None),
    ]);
    let roster = json!([
        {"ship_id": 33, "ship_type": "cargo_vessel", "owner": PLAYER},
        {"ship_id": 30, "ship_type": "cargo_vessel", "owner": PLAYER},
        {"ship_id": 31, "ship_type": "colony_ship", "owner": PLAYER},
    ]);
    reply(
        &w,
        "/planets/7",
        &planet(7, Some(PLAYER), buildings, builds, roster),
    );
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    let page = b.get("/planets/7").await;
    assert_eq!(page.status, 200, "{}", page.body);
    let body = &page.body;
    has(body, "Slots: 10 of 8 used");
    for (row, state) in [
        ("Drill #2", "running"),
        (
            "Refinery #3",
            "slowed: short of ore, or ingot storage is full",
        ),
        ("Drill #4", "halted: ore storage is full"),
        ("Refinery #5", "halted: no ore, or ingot storage is full"),
        ("Shipyard #15", "running"),
        // §4.6 E1: an operational building with no flow.
        ("Shipyard #16", "operational"),
        // E2: the status after the progress, only when it isn't running.
        (
            "Shipyard #17",
            "under construction: 42% (504.0 of 1,200.0 t), +100.0 t/h, done at 18:10:00 UTC (in 5 min 48 s); slowed: short of ingots",
        ),
        (
            "Refinery #18",
            "under construction: 0% (&lt; 0.1 of 250.0 t), waiting for ingots; halted: no ingots",
        ),
        (
            "Drill #12",
            "being demolished; the slot frees at 18:05:00 UTC (in 48 s)",
        ),
        (
            "Shipyard #19",
            "being demolished; the slot frees at 2026-10-21 06:00:00 UTC (in 11 h 55 min)",
        ),
    ] {
        has(
            body,
            &format!(r#"<th scope="row">{row}</th><td>{state}</td>"#),
        );
    }
    // A block per Shipyard operational or with builds, in view order; not #17 (neither).
    let at = |s: &str| body.find(s).unwrap_or_else(|| panic!("{s}"));
    assert!(at("<h3>Shipyard #19</h3>") < at("<h3>Shipyard #15</h3>"));
    assert!(at("<h3>Shipyard #15</h3>") < at("<h3>Shipyard #16</h3>"));
    lacks(body, "<h3>Shipyard #17</h3>");
    has(body, "<h3>Shipyard #16</h3><p>No ships building.</p>");
    for (build, ship, state) in [
        (
            20,
            "Colony Ship",
            "building: 30% (240.0 of 800.0 t), +80.0 t/h, done at 18:10:00 UTC (in 5 min 48 s)",
        ),
        (21, "Cargo Vessel", "waiting (1st in line)"),
        (
            22,
            "Cargo Vessel",
            "building: 30% (240.0 of 800.0 t), +80.0 t/h, done at 18:10:00 UTC (in 5 min 48 s); slowed: short of ingots",
        ),
        (23, "Colony Ship", "waiting (2nd in line)"),
        (
            24,
            "Cargo Vessel",
            "building: 30% (240.0 of 800.0 t), waiting for ingots; halted: no ingots",
        ),
    ] {
        has(
            body,
            &format!(r#"<th scope="row">build #{build}</th><td>{ship}</td><td>{state}</td>"#),
        );
    }
    // Your ships, grouped by type, ascending ID.
    has(
        body,
        "<h2>Your ships here</h2><ul><li>Colony Ship #31</li><li>Cargo Vessel #30</li><li>Cargo Vessel #33</li></ul>",
    );
    lacks(body, "<h2>Your fleets here</h2>");
    no_controls(body);
    insta::assert_snapshot!("planet_every_state", pretty(body));
}

/// D-049: someone else's planet shows its public state and only the viewer's own ships.
#[tokio::test]
async fn someone_else_s_planet_and_an_unowned_empty_one() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let buildings =
        json!([{"state": "operational", "id": 2, "type": "shipyard", "status": "running"}]);
    let builds = json!([{
        "id": 20, "shipyard_id": 2, "ship_type": "colony_ship", "state": "active", "status": "running",
        "delivered_g": 0, "cost_g": 800_000_000, "rate_g_per_h": 80_000_000, "eta": "2026-10-20T18:14:12.000Z",
    }]);
    // The server sends only the viewer's ships; one of another player's would be left out too.
    let roster = json!([
        {"ship_id": 30, "ship_type": "cargo_vessel", "owner": PLAYER},
        {"ship_id": 31, "ship_type": "colony_ship", "owner": OTHER},
    ]);
    reply(
        &w,
        "/planets/4",
        &planet(4, Some(OTHER), buildings, builds, roster),
    );
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    let page = b.get("/planets/4").await;
    assert_eq!(page.status, 200, "{}", page.body);
    let body = &page.body;
    has(
        body,
        &format!(r#"Owner: <span title="{OTHER}">player 7c8d9e0f</span> · Position"#),
    );
    lacks(body, "Owner: you");
    lacks(body, "(homeworld)");
    // The public queue, without buttons.
    has(
        body,
        r#"<th scope="row">build #20</th><td>Colony Ship</td>"#,
    );
    has(
        body,
        "<h2>Your ships here</h2><ul><li>Cargo Vessel #30</li></ul>",
    );
    lacks(body, "#31");
    no_controls(body);
    insta::assert_snapshot!("planet_non_owner", pretty(body));

    // The viewer's homeworld number owned by someone else isn't "(homeworld)": the line is
    // about the viewer's own planet.
    reply(
        &w,
        "/planets/7",
        &planet(7, Some(OTHER), json!([]), json!([]), json!([])),
    );
    let taken = b.get("/planets/7").await.body;
    has(&taken, "Owner: <span");
    lacks(&taken, "(homeworld)");

    let mut empty = planet(9, None, json!([]), json!([]), json!([]));
    empty["valid_until"] = Value::Null;
    empty["stocks"] = json!({"iron_ore": stock(0, 0, 10_000_000_000_i64), "iron_ingot": stock(99_999, 0, 5_000_000_000_i64)});
    reply(&w, "/planets/9", &empty);
    let page = b.get("/planets/9").await;
    let body = &page.body;
    has(
        body,
        "Owner: unowned · Position (150.0, −20.5, 999.9) · Slots: 0 of 8 used",
    );
    has(
        body,
        "As of 18:04:12 UTC. Nothing is scheduled to change these values.",
    );
    has(body, "<td>0 t of 10,000.0 t (empty)</td><td>0 t/h</td>");
    has(body, "<td>&lt; 0.1 t of 5,000.0 t</td>");
    has(body, "<h2>Buildings</h2><p>No buildings.</p>");
    for absent in [
        "<h2>Shipyards</h2>",
        "<h2>Your ships here</h2>",
        "<h2>Your fleets here</h2>",
    ] {
        lacks(body, absent);
    }
    insta::assert_snapshot!("planet_empty", pretty(body));
}

/// A stale view: times at or before `as_of` read without "in", never "in −3 s" or "in 0 s".
#[tokio::test]
async fn times_at_or_before_as_of_have_no_in() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let buildings = json!([
        {"state": "under_construction", "id": 17, "type": "drill", "status": "running",
         "delivered_g": 150_000_000, "cost_g": 150_000_000, "rate_g_per_h": 0, "eta": AS_OF},
        {"state": "demolishing", "id": 12, "type": "drill", "demolition_ends_at": "2026-10-20T18:04:09.000Z"},
    ]);
    reply(
        &w,
        "/planets/7",
        &planet(7, Some(PLAYER), buildings, json!([]), json!([])),
    );
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    let body = b.get("/planets/7").await.body;
    has(
        &body,
        "under construction: 100% (150.0 of 150.0 t), 0 t/h, done at 18:04:12 UTC</td>",
    );
    has(
        &body,
        "being demolished; the slot frees at 18:04:09 UTC</td>",
    );
    lacks(&body, "(in ");
}

#[tokio::test]
async fn a_planet_number_the_web_can_t_use_is_refused_before_any_call() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    for path in [
        "/planets/abc",
        "/planets/-1",
        "/planets/+4",
        "/planets/1.5",
        "/planets/18446744073709551616",
        "/planets/%37",
    ] {
        let from = count(&w);
        let page = b.get(path).await;
        assert_eq!(page.status, 400, "{path}: {}", page.body);
        assert!(reads_since(&w, from).is_empty(), "{path} called the API");
        has(
            &page.body,
            "<h1>No such planet</h1><p>That isn't a planet number. Planet numbers look like 12. (<code>invalid_query</code>)</p>",
        );
        has(
            &page.body,
            r#"<a href="/universe">Find a planet on the Universe page</a>"#,
        );
        if path == "/planets/abc" {
            insta::assert_snapshot!("planet_not_a_number", pretty(&page.body));
        }
    }
}

#[tokio::test]
async fn an_unknown_planet_is_its_own_page_at_404() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    w.api
        .reply("/planets/5", Reply::error(404, "unknown_planet"));
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    let page = b.get("/planets/5").await;
    assert_eq!(page.status, 404, "{}", page.body);
    has(
        &page.body,
        "<h1>No such planet</h1><p>There's no planet #5. Check the number on the Universe page. (<code>unknown_planet</code>)</p>",
    );
    insta::assert_snapshot!("planet_unknown", pretty(&page.body));
}

/// The first failing read decides the page, `/players/me` first; each is §5.5's retry page with
/// the API's status, or the expired notice for a refused token.
#[tokio::test]
async fn a_failed_read_on_the_planet_page() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    reply(&w, "/planets/7", &planet_home(Some(PLAYER)));
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    w.api
        .reply("/players/me/fleets", Reply::error(500, "internal"));
    let page = b.get("/planets/7").await;
    assert_eq!(page.status, 500, "{}", page.body);
    has(
        &page.body,
        "<h1>Couldn't load this page</h1><p>Something went wrong on the game server. Try again. (<code>internal</code>)</p><p><a href=\"/planets/7\">Try again</a></p>",
    );
    insta::assert_snapshot!("planet_retry", pretty(&page.body));

    // `/players/me` decides before the page's own failures.
    w.api.reply(
        "/players/me",
        Reply::error(503, "busy").header("retry-after", "4"),
    );
    w.api.reply("/planets/7", Reply::error(429, "rate_limited"));
    let page = b.get("/planets/7").await;
    assert_eq!(page.status, 503, "{}", page.body);
    has(
        &page.body,
        "The game is busy. Try again in 4 seconds. (<code>busy</code>)",
    );

    // A refused token ends the session, and the login returns here.
    w.api
        .reply("/players/me", Reply::error(401, "invalid_token"));
    let page = b.get("/planets/7").await;
    assert_eq!(page.status, 200, "{}", page.body);
    has(&page.body, "Your login has expired. Log in again.");
    has(
        &page.body,
        r#"<a href="/login?next=%2Fplanets%2F7">Log in</a>"#,
    );
    // The login ended: the old session is gone, and the page logged out.
    w.api
        .reply("/players/me", Reply::json(StatusCode::OK, &player_view(42)));
    let again = b.get("/planets/7").await;
    has(
        &again.body,
        r#"<a href="/login?next=%2Fplanets%2F7">Log in</a>"#,
    );
    lacks(&again.body, "Logged in as");
}

const RENEWED: &str = "<p><strong>Your session had ended; you're logged in again.</strong></p>";

/// §5.6 (carried by D-059, ruled by the lead): after an ended session, logging in again shows
/// the line on the next page, once; the flag is the web's own, never a URL's.
#[tokio::test]
async fn logging_in_again_after_an_ended_session_says_so_once() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    reply(&w, "/leaderboard", &board(json!([])));
    // A normal login shows nothing.
    lacks(&b.get("/").await.body, "Your session had ended");
    w.api
        .reply("/players/me", Reply::error(401, "invalid_token"));
    let ended = b.get("/fleets").await;
    has(&ended.body, "Your login has expired. Log in again.");
    w.api
        .reply("/players/me", Reply::json(StatusCode::OK, &player_view(42)));
    // The login link is the plain one: nothing in it carries the flag.
    has(&ended.body, r#"<a href="/login?next=%2Ffleets">Log in</a>"#);
    let callback = b.start_login("/login?next=%2Ffleets").await;
    let back = b.get(&callback).await;
    assert_eq!(
        (back.status, back.location.as_deref()),
        (303, Some("/fleets"))
    );
    let cookies = b.set_cookies.len();
    let first = b.get("/fleets").await;
    assert_eq!(first.status, 200, "{}", first.body);
    // Removing the notice saved the session, keeping its login's lifetime (keep_expiry), not
    // the anonymous 10 minutes.
    let set: Vec<String> = b.set_cookies[cookies..]
        .iter()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let max_age: Option<u64> = set
        .first()
        .and_then(|c| c.split("max-age=").nth(1))
        .and_then(|v| v.split(';').next()?.trim().parse().ok());
    // The token's hour plus the 24 h grace, well past an anonymous session's 600 s.
    assert!(
        set.len() == 1 && max_age.is_some_and(|a| a > 24 * 3600),
        "{set:?}"
    );
    has(&first.body, &format!("<hr>{RENEWED}<h1>Your fleets</h1>"));
    // Exactly once: the next page, and every page after, doesn't have it.
    for path in ["/fleets", "/", "/leaderboard"] {
        lacks(&b.get(path).await.body, "Your session had ended");
    }
}

#[tokio::test]
async fn a_url_can_t_ask_for_the_line() {
    let w = World::start().await;
    let mut b = w.browser();
    for login in [
        "/login?renew",
        "/login?renew=1&next=%2F",
        "/login?notice=true",
    ] {
        let callback = b.start_login(login).await;
        assert_eq!(b.get(&callback).await.status, 303);
        lacks(&b.get("/").await.body, "Your session had ended");
    }
}

/// T1: a page's reads run at once, so a slow API holds it for one read's time, not four.
#[tokio::test]
async fn a_page_s_reads_run_at_once() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let slow = |v: &Value| Reply::json(StatusCode::OK, v).delay(600);
    w.api.reply("/planets/7", slow(&planet_home(Some(PLAYER))));
    w.api
        .reply("/players/me/fleets", slow(&fleets_view(json!([]), None)));
    w.api.reply("/players/me", slow(&player_view(42)));
    *w.api.rules.lock().unwrap() = slow(&rules_view("v0"));
    let started = Instant::now();
    let page = b.get("/planets/7").await;
    let took = started.elapsed();
    assert_eq!(page.status, 200, "{}", page.body);
    // One after another would take 2.4 s.
    assert!(took < Duration::from_millis(1_200), "{took:?}");
}

// ---------------------------------------------------------------------------------------------
// Fleets (§4.3)

#[tokio::test]
async fn fleets_in_every_state_in_their_order() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let mut me = player_view(3450);
    me["planet_ids"] = json!([7, 9]);
    w.api.reply("/players/me", Reply::json(StatusCode::OK, &me));
    let in_transit = json!({
        "state": "in_transit", "fleet_id": 8, "ships": ships(&[(41, "colony_ship")]),
        "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}, "capacity_g": 0, "speed_du_per_h": 10_000,
        "origin_planet_id": 7, "target_planet_id": 12, "mission": "colonize",
        "departed_at": "2026-10-20T18:00:00.000Z", "arrives_at": "2026-10-20T18:20:00.000Z",
    });
    let returning = json!({
        "state": "returning", "fleet_id": 3, "ships": ships(&[(50, "cargo_vessel")]),
        "cargo": {"iron_ore_g": 250_000_000, "iron_ingot_g": 0}, "capacity_g": 500_000_000,
        "speed_du_per_h": 15_000, "origin_planet_id": 7, "arrives_at": "2026-10-20T18:07:00.000Z",
    });
    let list = json!([
        returning,
        in_transit,
        // At your planet, with cargo: unload is P4.4's; disband says why it isn't offered.
        stationed(
            7,
            9,
            ships(&[
                (33, "cargo_vessel"),
                (30, "colony_ship"),
                (31, "cargo_vessel")
            ]),
            250_000_000,
            500_000_000,
            1_000_000_000
        ),
        // Not at your planet, with cargo: both lines.
        stationed(
            9,
            12,
            ships(&[(60, "cargo_vessel")]),
            0,
            100_000_000,
            500_000_000
        ),
        // No cargo, some capacity (§4.6 E6), and none.
        stationed(5, 7, ships(&[(61, "cargo_vessel")]), 0, 0, 500_000_000),
        stationed(6, 7, ships(&[(62, "colony_ship")]), 0, 0, 0),
    ]);
    reply(
        &w,
        "/players/me/fleets",
        &fleets_view(list, Some("2026-10-20T18:07:00.000Z")),
    );
    let from = count(&w);
    let page = b.get("/fleets").await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert_eq!(
        reads_since(&w, from),
        reads(&["/players/me", "/players/me/fleets", "/rules"])
    );
    let body = &page.body;
    // Without the other-player clause (§2).
    has(
        body,
        r#"As of 18:04:12 UTC. These values hold until 18:07:00 UTC. <a href="/fleets">Refresh</a>"#,
    );
    // Stationed, in transit, returning; by ID within each.
    let order: Vec<usize> = [5, 6, 7, 9, 8, 3]
        .iter()
        .map(|id| {
            body.find(&format!(r#"<h2 id="fleet-{id}">"#))
                .unwrap_or_else(|| panic!("fleet {id}"))
        })
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    for want in [
        r#"<h2 id="fleet-7">Fleet #7: stationed at <a href="/planets/9">Planet #9</a></h2>"#,
        "<p>Ships: 1 Colony Ship, 2 Cargo Vessels (#30, #31, #33)</p>",
        "<p>Cargo: 250.0 t of ore, 500.0 t of ingots (750.0 t of 1,000.0 t capacity) · Speed 10.0 u/h</p>",
        r#"<h2 id="fleet-8">Fleet #8: flying to <a href="/planets/12">Planet #12</a> (Colonize)</h2><p>Left <a href="/planets/7">Planet #7</a> at 18:00:00 UTC; arrives 18:20:00 UTC (in 15 min 48 s)</p>"#,
        "<p>Cargo: none · Speed 10.0 u/h</p>",
        r#"<h2 id="fleet-3">Fleet #3: returning to <a href="/planets/7">Planet #7</a></h2><p>Arrives back 18:07:00 UTC (in 2 min 48 s)</p>"#,
        "<p>Already turned back; it can't be recalled again.</p>",
        "<p>Cargo: none (500.0 t capacity) · Speed 10.0 u/h</p>",
    ] {
        has(body, want);
    }
    let section = |id: u64| {
        let start = body.find(&format!(r#"<h2 id="fleet-{id}">"#)).unwrap();
        &body[start..start + body[start..].find("</section>").unwrap()]
    };
    let move_line = "Move this fleet to one of your planets to unload its cargo.";
    let unload_line = "Unload the cargo before disbanding.";
    assert!(section(7).contains(unload_line) && !section(7).contains(move_line));
    assert!(section(9).contains(unload_line) && section(9).contains(move_line));
    for id in [5, 6, 8, 3] {
        assert!(
            !section(id).contains(unload_line) && !section(id).contains(move_line),
            "{id}"
        );
    }
    has(
        body,
        r#"<nav><p><strong>Voidforge</strong> <a href="/">Home</a> · Fleets · <a href="/universe">Universe</a>"#,
    );
    no_controls(body);
    insta::assert_snapshot!("fleets_every_state", pretty(body));

    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    let body = b.get("/fleets").await.body;
    has(
        &body,
        "<p>You have no fleets. Assemble one from ships on a planet's roster.</p>",
    );
    has(
        &body,
        "As of 18:04:12 UTC. Nothing is scheduled to change these values.",
    );
    insta::assert_snapshot!("fleets_empty", pretty(&body));
}

/// The server's own route snapshot of a fleet in transit (`fleets_bob.json`).
#[tokio::test]
async fn fleets_from_the_server_s_own_view() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let view: Value = serde_json::from_str(include_str!("fixtures/fleets_bob.json")).unwrap();
    reply(&w, "/players/me/fleets", &view);
    let body = b.get("/fleets").await.body;
    has(
        &body,
        r#"Fleet #11: flying to <a href="/planets/7">Planet #7</a> (Colonize)</h2><p>Left <a href="/planets/4">Planet #4</a> at 08:53:20 UTC; arrives 09:53:20 UTC (in 1 h)</p><p>Ships: 1 Colony Ship (#10)</p><p>Cargo: none · Speed 10.0 u/h</p>"#,
    );
}

#[tokio::test]
async fn a_failed_fleets_read_is_the_retry_page() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    w.api
        .reply("/players/me/fleets", Reply::raw(StatusCode::OK, "{}"));
    let page = b.get("/fleets").await;
    assert_eq!(page.status, 502, "{}", page.body);
    has(
        &page.body,
        "The game server answered something unexpected. This is a bug; please report it.</p>",
    );
}

// ---------------------------------------------------------------------------------------------
// Universe (§4.4)

fn near(planets: Value) -> Value {
    json!({"as_of": AS_OF, "valid_until": null, "planets": planets})
}

fn summary(id: u64, owner: Option<&str>, distance: i64, x: i64) -> Value {
    json!({"planet_id": id, "owner": owner, "distance": distance, "position": {"x": x, "y": -20_500, "z": 0}})
}

/// The default: the homeworld's position, then the planets within 100 u of it (Q5).
#[tokio::test]
async fn the_universe_around_your_homeworld() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    reply(
        &w,
        "/planets/7",
        &planet(7, Some(PLAYER), json!([]), json!([]), json!([])),
    );
    reply(
        &w,
        "/planets",
        &near(json!([
            summary(12, Some(OTHER), 40_000, 190_000),
            summary(11, None, 32_100, 182_100),
            summary(7, Some(PLAYER), 0, 150_000),
            summary(10, None, 40_000, 110_000),
        ])),
    );
    let from = count(&w);
    let page = b.get("/universe").await;
    assert_eq!(page.status, 200, "{}", page.body);
    // The homeworld's position comes from its own read, in distance units.
    assert_eq!(
        reads_since(&w, from),
        reads(&[
            "/planets/7",
            "/planets?near=150000,-20500,999999&radius=100000",
            "/players/me",
            "/rules"
        ])
    );
    let body = &page.body;
    has(
        body,
        r#"Planets within 100.0 u of (150.0, −20.5, 999.9), around <a href="/planets/7">Planet #7</a>."#,
    );
    has(
        body,
        r#"As of 18:04:12 UTC. Nothing is scheduled to change these values. <a href="/universe">Refresh</a>"#,
    );
    // D1: the form's values use the hyphen, and read back to the same search.
    for field in [
        r#"name="x" value="150.0""#,
        r#"name="y" value="-20.5""#,
        r#"name="z" value="999.999""#,
        r#"name="radius" value="100.0""#,
    ] {
        has(body, field);
    }
    has(body, r#"<form method="get" action="/universe">"#);
    // By distance, then planet number (the API promises no order).
    let rows: Vec<usize> = [7, 11, 10, 12]
        .iter()
        .map(|id| {
            body.find(&format!(r#"<th scope="row"><a href="/planets/{id}">"#))
                .unwrap()
        })
        .collect();
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "{rows:?}");
    has(
        body,
        r#"Planet #7</a></th><td>you (homeworld)</td><td>0.0 u</td><td>(150.0, −20.5, 0.0)</td>"#,
    );
    has(
        body,
        r#"Planet #11</a></th><td>unowned</td><td>32.1 u</td>"#,
    );
    has(
        body,
        &format!(
            r#"Planet #12</a></th><td><span title="{OTHER}">player 7c8d9e0f</span></td><td>40.0 u</td>"#
        ),
    );
    has(
        body,
        r#"<strong>Voidforge</strong> <a href="/">Home</a> · <a href="/fleets">Fleets</a> · Universe · "#,
    );
    insta::assert_snapshot!("universe_default", pretty(body));
}

/// An explicit search goes straight to `/planets?near`, beside `/players/me`, in exact units.
#[tokio::test]
async fn an_explicit_search_converts_exactly_and_accepts_both_minus_signs() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    reply(&w, "/planets", &near(json!([])));
    for (query, want) in [
        (
            "x=150&y=%E2%88%9220.5&z=0.001&radius=5",
            "/planets?near=150000,-20500,1&radius=5000",
        ),
        (
            "x=-20.5&y=0&z=.5&radius=0",
            "/planets?near=-20500,0,500&radius=0",
        ),
    ] {
        let from = count(&w);
        let page = b.get(&format!("/universe?{query}")).await;
        assert_eq!(page.status, 200, "{query}: {}", page.body);
        assert_eq!(
            reads_since(&w, from),
            reads(&[want, "/players/me", "/rules"]),
            "{query}"
        );
    }
    let body = b.get("/universe?x=150&y=-20.5&z=0.001&radius=5").await.body;
    has(&body, "Planets within 5.0 u of (150.0, −20.5, 0.0).</p>");
    has(&body, "<p>No planets within 5.0 u of that point.</p>");
    has(&body, r#"name="z" value="0.001""#);
}

#[tokio::test]
async fn around_a_planet_reads_it_beside_players_me() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let mut p4 = planet(4, Some(OTHER), json!([]), json!([]), json!([]));
    p4["position"] = json!({"x": -1, "y": 2, "z": 3});
    reply(&w, "/planets/4", &p4);
    reply(
        &w,
        "/planets",
        &near(json!([summary(4, Some(OTHER), 0, -1)])),
    );
    w.api.reply(
        "/players/me",
        Reply::json(StatusCode::OK, &player_view(1)).delay(400),
    );
    w.api
        .reply("/planets/4", Reply::json(StatusCode::OK, &p4).delay(400));
    let from = count(&w);
    let started = Instant::now();
    let page = b.get("/universe?around=4").await;
    // Side by side: one after the other would take 0.8 s.
    assert!(
        started.elapsed() < Duration::from_millis(750),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(page.status, 200, "{}", page.body);
    assert_eq!(
        reads_since(&w, from),
        reads(&[
            "/planets/4",
            "/planets?near=-1,2,3&radius=100000",
            "/players/me",
            "/rules"
        ])
    );
    has(
        &page.body,
        r#"Planets within 100.0 u of (0.0, 0.0, 0.0), around <a href="/planets/4">Planet #4</a>."#,
    );
    has(&page.body, r#"name="x" value="-0.001""#);

    w.api
        .reply("/planets/5", Reply::error(404, "unknown_planet"));
    let page = b.get("/universe?around=5").await;
    assert_eq!(page.status, 404, "{}", page.body);
    has(
        &page.body,
        "There's no planet #5. Check the number on the Universe page. (<code>unknown_planet</code>)",
    );
}

#[tokio::test]
async fn a_search_the_web_can_t_use_is_refused_before_any_call() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    for query in [
        "x=abc&y=0&z=0&radius=1",
        "x=0&y=0&z=0&radius=-1",
        "x=0&y=0&z=0&radius=%E2%88%921",
        "x=0&y=0&z=0",
        "x=1.2345&y=0&z=0&radius=1",
        "radius=5",
        "x=1,000&y=0&z=0&radius=1",
        "around=abc",
        "around=-4",
        "around=",
        "x=9223372036854776&y=0&z=0&radius=1",
    ] {
        let from = count(&w);
        let page = b.get(&format!("/universe?{query}")).await;
        assert_eq!(page.status, 400, "{query}: {}", page.body);
        assert!(reads_since(&w, from).is_empty(), "{query} called the API");
        has(
            &page.body,
            "<p><strong>Enter numbers like 150 or −20.5; the radius must be 0 or more.</strong></p>",
        );
    }
    // The form keeps what was typed, escaped.
    let page = b.get("/universe?x=%3Cb%3E&y=1&z=2&radius=3").await;
    has(&page.body, r#"name="x" value="&lt;b&gt;""#);
    lacks(&page.body, "<b>");
    insta::assert_snapshot!("universe_refused_input", pretty(&page.body));
}

/// §4.4: the numbers were checked, so `invalid_query` from the search means too many planets.
#[tokio::test]
async fn too_many_planets_keeps_the_form() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    w.api.reply("/planets", Reply::error(400, "invalid_query"));
    let page = b.get("/universe?x=0&y=0&z=0&radius=1000000").await;
    assert_eq!(page.status, 400, "{}", page.body);
    has(
        &page.body,
        "<p><strong>Too many planets in that area (more than 1,000). Use a smaller radius.</strong> (<code>invalid_query</code>)</p>",
    );
    has(&page.body, r#"name="radius" value="1000000.0""#);
    lacks(&page.body, "<table");
    insta::assert_snapshot!("universe_too_many", pretty(&page.body));

    w.api.reply("/planets", Reply::error(503, "unavailable"));
    let page = b.get("/universe?x=0&y=0&z=0&radius=1").await;
    assert_eq!(page.status, 503, "{}", page.body);
    has(&page.body, "<h1>Couldn't load this page</h1>");
}

// ---------------------------------------------------------------------------------------------
// Leaderboard (§4.5)

fn board(entries: Value) -> Value {
    json!({"as_of": "2026-10-20T18:03:30.000Z", "entries": entries})
}

#[tokio::test]
async fn the_leaderboard_marks_you_and_shares_tied_ranks() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    let third = "01928c5e-7a3b-7c2d-9e4f-4b1a07c3aaaa";
    reply(
        &w,
        "/leaderboard",
        &board(json!([
            {"rank": 1, "player_id": third, "score": 5_200},
            {"rank": 2, "player_id": PLAYER, "score": 3_450},
            {"rank": 2, "player_id": OTHER, "score": 3_450},
            {"rank": 4, "player_id": "01928c5e-7a3b-7c2d-9e4f-000000000004", "score": -1_200},
        ])),
    );
    let from = count(&w);
    let page = b.get("/leaderboard").await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert_eq!(
        reads_since(&w, from),
        reads(&["/leaderboard", "/players/me", "/rules"])
    );
    let body = &page.body;
    has(
        body,
        r#"Ranked as of 18:03:30 UTC. The board is rebuilt every minute; your score in the header is current. <a href="/leaderboard">Refresh</a>"#,
    );
    has(body, "<p>You're ranked 2 of 4.</p>");
    has(
        body,
        "<tr><td><strong>2</strong></td><td><strong>you</strong></td><td><strong>3,450</strong></td></tr>",
    );
    has(
        body,
        &format!(
            r#"<tr><td>2</td><td><span title="{OTHER}">player 7c8d9e0f</span></td><td>3,450</td></tr>"#
        ),
    );
    has(body, "<td>−1,200</td>");
    // Rank as given: the tie keeps its order and the next rank is 4.
    assert!(
        body.find("<strong>you</strong>").unwrap()
            < body.find("player 7c8d9e0f</span></td><td>3,450").unwrap()
    );
    has(
        body,
        r#"<a href="/universe">Universe</a> · Leaderboard · You:"#,
    );
    insta::assert_snapshot!("leaderboard", pretty(body));

    // §4.6 E5: joined since the last build.
    reply(
        &w,
        "/leaderboard",
        &board(json!([{"rank": 1, "player_id": OTHER, "score": 10}])),
    );
    let body = b.get("/leaderboard").await.body;
    has(
        &body,
        "<p>You're not on the board yet; it's rebuilt every minute.</p>",
    );
    lacks(&body, "<strong>you</strong>");
}

#[tokio::test]
async fn before_its_first_build_the_leaderboard_says_so() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    for (retry, copy) in [
        (
            "1",
            "The leaderboard isn't ready yet. Try again in a moment.",
        ),
        (
            "5",
            "The leaderboard isn't ready yet. Try again in 5 seconds.",
        ),
    ] {
        w.api.reply(
            "/leaderboard",
            Reply::error(503, "unavailable").header("retry-after", retry),
        );
        let r = reqwest::Client::new()
            .get(format!("{}/leaderboard", w.web))
            .header(
                "cookie",
                format!(
                    "{}={}",
                    voidforge_web::routes::COOKIE_NAME,
                    b.cookie.clone().unwrap()
                ),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 503);
        assert_eq!(r.headers()["retry-after"], retry);
        let body = r.text().await.unwrap();
        has(
            &body,
            &format!(
                "<p>{copy} (<code>unavailable</code>)</p><p><a href=\"/leaderboard\">Try again</a></p>"
            ),
        );
        if retry == "1" {
            insta::assert_snapshot!("leaderboard_not_ready", pretty(&body));
        }
    }
    // Another 503 keeps the general copy.
    w.api.reply("/leaderboard", Reply::error(503, "busy"));
    has(
        &b.get("/leaderboard").await.body,
        "The game is busy. Try again in a moment. (<code>busy</code>)",
    );
}

// ---------------------------------------------------------------------------------------------
// Every page

/// §4.6 E4: logged out, each page is the logged-out Home at its URL, Log in returning there,
/// and nothing but the public `/rules` is read.
#[tokio::test]
async fn logged_out_each_page_offers_a_login_back_to_it() {
    let w = World::start().await;
    let mut b = w.browser();
    for (path, next) in [
        ("/planets/7", "%2Fplanets%2F7"),
        ("/fleets", "%2Ffleets"),
        ("/universe?around=4", "%2Funiverse%3Faround%3D4"),
        ("/leaderboard", "%2Fleaderboard"),
    ] {
        let from = count(&w);
        let page = b.get(path).await;
        assert_eq!(page.status, 200, "{path}");
        has(
            &page.body,
            &format!(r#"<a href="/login?next={next}">Log in</a>"#),
        );
        lacks(&page.body, "<nav>");
        assert_eq!(reads_since(&w, from), reads(&["/rules"]), "{path}");
        assert_eq!(b.cookie, None, "{path} made a session");
    }
}

#[tokio::test]
async fn the_fast_rules_line_is_on_every_page() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    *w.api.rules.lock().unwrap() = Reply::json(StatusCode::OK, &rules_view("v0-fast"));
    reply(&w, "/planets/7", &planet_home(Some(PLAYER)));
    reply(&w, "/players/me/fleets", &fleets_view(json!([]), None));
    reply(&w, "/planets", &near(json!([])));
    reply(&w, "/leaderboard", &board(json!([])));
    for path in ["/planets/7", "/fleets", "/universe", "/leaderboard"] {
        has(
            &b.get(path).await.body,
            "<strong>Fast test rules:</strong> an hour of play takes a second.",
        );
    }
    // A failed `/rules` only drops the line.
    *w.api.rules.lock().unwrap() = Reply::error(500, "internal");
    for path in ["/planets/7", "/fleets", "/universe", "/leaderboard"] {
        let page = b.get(path).await;
        assert_eq!(page.status, 200, "{path}: {}", page.body);
        lacks(&page.body, "Fast test rules");
    }
}

#[tokio::test]
async fn any_other_path_is_not_found_with_no_call() {
    let w = World::start().await;
    let mut b = logged_in(&w).await;
    for path in [
        "/nowhere",
        "/planets",
        "/planets/",
        "/planets/7/x",
        "/fleets/7",
    ] {
        let from = count(&w);
        let page = b.get(path).await;
        assert_eq!(page.status, 404, "{path}");
        has(
            &page.body,
            r#"<h1>Not found</h1><p>There's no page here. <a href="/">Home</a></p>"#,
        );
        assert!(reads_since(&w, from).is_empty(), "{path}");
    }
    insta::assert_snapshot!("not_found", pretty(&b.get("/nowhere").await.body));
}
