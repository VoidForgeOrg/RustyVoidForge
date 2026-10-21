//! The command forms by effect through the router against the API stand-in (P4.4, D-061):
//! every action's one `POST /commands` (phase4-ux §5.3), each outcome class (§5.1, §5.5, §6),
//! the form-key lifecycle (§5.2, Q3, the lead's T1-T7), the web's own checks (§6.3) and the
//! confirm pages (§5.4).
//!
//! Each test asserts what reached the API (the exact body, the `Idempotency-Key`, how many
//! calls) beside what the player sees, since a banner alone can't show that an order wasn't
//! sent twice.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{Browser, Options, Page, Reply, World, now, urlencode};
use serde_json::{Value, json};
use voidforge_web::forms::{
    ACTION_EXPIRED, ALREADY_SUBMITTED, EXPIRED_FORM, NOT_LOGGED_IN, ORDER_NOT_SENT,
};
use voidforge_web::messages::UNREACHABLE_COMMAND;
use voidforge_web::pages::RENEWED;
use voidforge_web::session::MAX_FORM_KEYS;

const AS_OF: &str = "2026-10-20T18:04:12.000Z";

async fn logged_in(w: &World) -> Browser {
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    b
}

fn reply(w: &World, path: &str, body: &Value) {
    w.api.reply(path, Reply::json(StatusCode::OK, body));
}

fn has(body: &str, want: &str) {
    assert!(body.contains(want), "missing {want:?} in:\n{body}");
}

fn lacks(body: &str, unwanted: &str) {
    assert!(
        !body.contains(unwanted),
        "unexpected {unwanted:?} in:\n{body}"
    );
}

/// The form key a page was rendered with; every form on it carries the same one (T1).
fn key_of(body: &str) -> String {
    let marker = r#"name="key" value=""#;
    let keys: Vec<&str> = body
        .match_indices(marker)
        .map(|(i, _)| {
            let rest = &body[i + marker.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert!(!keys.is_empty(), "no form key in:\n{body}");
    assert!(
        keys.iter().all(|k| *k == keys[0]),
        "one key per render: {keys:?}"
    );
    keys[0].to_owned()
}

/// A page's hidden fields, in order, as a retry page re-posts them.
fn hidden_fields(body: &str) -> Vec<(String, String)> {
    let marker = r#"<input type="hidden" name=""#;
    body.match_indices(marker)
        .map(|(i, _)| {
            let rest = &body[i + marker.len()..];
            let name = &rest[..rest.find('"').unwrap()];
            let rest = &rest[rest.find(r#"value=""#).unwrap() + 7..];
            (name.to_owned(), rest[..rest.find('"').unwrap()].to_owned())
        })
        .collect()
}

async fn submit(b: &mut Browser, fields: &[(&str, &str)]) -> Page {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let req = b
        .http
        .post(format!("{}/act", b.web))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body);
    b.send(req).await
}

fn commands(w: &World) -> Vec<common::Request> {
    w.api.requests_to("/commands")
}

fn accepted(events: &Value) -> Reply {
    Reply::json(
        StatusCode::OK,
        &json!({"seq": 2, "game_time": AS_OF, "events": events}),
    )
}

fn stock(amount_g: i64) -> Value {
    json!({"amount_g": amount_g, "rate_g_per_h": 0, "cap_g": 5_000_000_000_i64})
}

/// Planet 7, the viewer's: a Shipyard under construction (#15, 504 t delivered), an operational
/// Drill (#12), an operational Shipyard with a build (#16, build #20 with 240 t), and a Drill
/// being demolished (#13).
fn home_planet(owner: &str) -> Value {
    json!({
        "planet_id": 7, "owner": owner, "as_of": AS_OF, "valid_until": null,
        "stocks": {"iron_ore": stock(2_000_000_000), "iron_ingot": stock(1_500_000_000)},
        "buildings": [
            {"state": "under_construction", "id": 15, "type": "shipyard", "status": "running",
             "delivered_g": 504_000_000, "cost_g": 1_200_000_000, "rate_g_per_h": 100_000_000,
             "eta": "2026-10-20T18:10:00.000Z"},
            {"state": "operational", "id": 12, "type": "drill", "status": "running"},
            {"state": "operational", "id": 16, "type": "shipyard", "status": "running"},
            {"state": "demolishing", "id": 13, "type": "drill", "status": null,
             "demolition_ends_at": "2026-10-20T18:05:00.000Z"},
        ],
        "position": {"x": 0, "y": 0, "z": 0}, "slots": 8,
        "ship_builds": [
            {"id": 20, "shipyard_id": 16, "ship_type": "colony_ship", "state": "active", "status": "running",
             "delivered_g": 240_000_000, "cost_g": 800_000_000, "rate_g_per_h": 80_000_000,
             "eta": "2026-10-20T18:11:00.000Z"},
        ],
        "roster": [
            {"ship_id": 30, "ship_type": "cargo_vessel", "owner": common::PLAYER},
            {"ship_id": 31, "ship_type": "colony_ship", "owner": common::PLAYER},
        ],
    })
}

fn stationed(id: u64, at: u64, ingots: i64) -> Value {
    json!({
        "state": "stationed", "fleet_id": id, "ships": [{"ship_id": 30, "ship_type": "cargo_vessel"}],
        "cargo": {"iron_ore_g": 0, "iron_ingot_g": ingots}, "capacity_g": 500_000_000,
        "speed_du_per_h": 15_000, "at_planet_id": at,
    })
}

/// Fleet 21 stationed at 7 with ingots, fleet 22 flying 7 → 12 to colonize, fleet 23 returning,
/// fleet 24 stationed at 9 with nothing. Fleet 22 left 62.8 s before `as_of`: shown, the two
/// times are 62 s apart (18:03:10 and 18:04:12), so the confirm says `1 min 2 s`; from the raw
/// times it would round up to `1 min 3 s` (dev-design's R1 rule).
fn my_fleets() -> Value {
    json!({"as_of": "2026-10-20T18:04:12.900Z", "valid_until": null, "fleets": [
        stationed(21, 7, 500_000_000),
        {"state": "in_transit", "fleet_id": 22, "ships": [{"ship_id": 40, "ship_type": "colony_ship"}],
         "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}, "capacity_g": 0, "speed_du_per_h": 10_000,
         "origin_planet_id": 7, "target_planet_id": 12, "mission": "colonize",
         "departed_at": "2026-10-20T18:03:10.100Z", "arrives_at": "2026-10-20T18:20:00.000Z"},
        {"state": "returning", "fleet_id": 23, "ships": [{"ship_id": 41, "ship_type": "colony_ship"}],
         "cargo": {"iron_ore_g": 0, "iron_ingot_g": 0}, "capacity_g": 0, "speed_du_per_h": 10_000,
         "origin_planet_id": 7, "arrives_at": "2026-10-20T18:07:00.000Z"},
        stationed(24, 9, 0),
    ]})
}

/// A world with planet 7 and the fleets above scripted, and a logged-in browser.
async fn world() -> (World, Browser) {
    let w = World::start().await;
    reply(&w, "/planets/7", &home_planet(common::PLAYER));
    reply(&w, "/players/me/fleets", &my_fleets());
    let b = logged_in(&w).await;
    (w, b)
}

/// A fresh key, from a render of the Fleets page.
async fn fresh_key(b: &mut Browser) -> String {
    let page = b.get("/fleets").await;
    assert_eq!(page.status, 200, "{}", page.body);
    key_of(&page.body)
}

// ---------------------------------------------------------------------------------------------
// Every action (§5.3)

/// The fields sent, the command body the API must get, the events it answers, where the browser
/// goes, and the banner there.
type Case<'a> = (Vec<(&'a str, &'a str)>, Value, Value, &'a str, &'a str);

#[tokio::test]
async fn every_action_sends_its_one_command_and_says_done() {
    let (w, mut b) = world().await;
    let cases: Vec<Case<'_>> = vec![
        (
            vec![
                ("action", "build_building"),
                ("return_to", "/planets/7"),
                ("planet_id", "7"),
                ("building_type", "shipyard"),
            ],
            json!({"type": "build_building", "payload": {"planet_id": 7, "building_type": "shipyard"}}),
            json!([]),
            "/planets/7",
            "Done: construction of a Shipyard started on Planet #7. It draws ingots until it's complete.",
        ),
        (
            vec![
                ("action", "cancel_construction"),
                ("return_to", "/planets/7"),
                ("building_id", "15"),
                ("building_type", "shipyard"),
            ],
            json!({"type": "cancel_construction", "payload": {"building_id": 15}}),
            json!([]),
            "/planets/7",
            "Done: construction of Shipyard #15 cancelled. Its slot is free; the ingots delivered are lost.",
        ),
        (
            vec![
                ("action", "demolish_building"),
                ("return_to", "/planets/7"),
                ("building_id", "12"),
                ("building_type", "drill"),
            ],
            json!({"type": "demolish_building", "payload": {"building_id": 12}}),
            json!([]),
            "/planets/7",
            "Done: Drill #12 is being demolished. It has stopped working; its slot frees when demolition ends.",
        ),
        (
            vec![
                ("action", "queue_ship"),
                ("return_to", "/planets/7"),
                ("shipyard_id", "16"),
                ("ship_type", "colony_ship"),
            ],
            json!({"type": "queue_ship", "payload": {"shipyard_id": 16, "ship_type": "colony_ship"}}),
            json!([]),
            "/planets/7",
            "Done: a Colony Ship was queued at Shipyard #16.",
        ),
        (
            vec![
                ("action", "cancel_ship_build"),
                ("return_to", "/planets/7"),
                ("ship_build_id", "20"),
            ],
            json!({"type": "cancel_ship_build", "payload": {"ship_build_id": 20}}),
            json!([]),
            "/planets/7",
            "Done: build #20 cancelled. The ingots delivered are lost.",
        ),
        (
            vec![
                ("action", "assemble_fleet"),
                ("return_to", "/planets/7"),
                ("planet_id", "7"),
                ("ship", "31"),
                ("ship", "30"),
                ("ore", ""),
                ("ingots", "500.25"),
            ],
            json!({"type": "assemble_fleet", "payload": {"planet_id": 7, "ship_ids": [31, 30],
                "cargo": {"iron_ore_g": 0, "iron_ingot_g": 500_250_000}}}),
            json!([]),
            "/fleets",
            "Done: a fleet of 2 ships was assembled at Planet #7.",
        ),
        (
            vec![
                ("action", "send_fleet"),
                ("return_to", "/planets/9"),
                ("fleet_id", "24"),
                ("target_planet_id", "12"),
                ("mission", "colonize"),
            ],
            json!({"type": "send_fleet", "payload": {"fleet_id": 24, "target_planet_id": 12, "mission": "colonize"}}),
            json!([]),
            "/fleets",
            "Done: Fleet #24 left for Planet #12 (Colonize).",
        ),
        (
            vec![
                ("action", "recall_fleet"),
                ("return_to", "/fleets"),
                ("fleet_id", "22"),
                ("origin", "7"),
            ],
            json!({"type": "recall_fleet", "payload": {"fleet_id": 22}}),
            json!([]),
            "/fleets",
            "Done: Fleet #22 turned back to Planet #7.",
        ),
        (
            vec![
                ("action", "disband_fleet"),
                ("return_to", "/fleets"),
                ("fleet_id", "24"),
                ("planet_id", "9"),
            ],
            json!({"type": "disband_fleet", "payload": {"fleet_id": 24}}),
            json!([]),
            "/fleets",
            "Done: Fleet #24 disbanded. Its ships are on Planet #9's roster.",
        ),
        (
            vec![
                ("action", "trigger_unload"),
                ("return_to", "/fleets"),
                ("fleet_id", "21"),
                ("planet_id", "7"),
                ("aboard_ore_g", "0"),
                ("aboard_ingots_g", "500000000"),
            ],
            json!({"type": "trigger_unload", "payload": {"fleet_id": 21}}),
            // As the core answers it: no event of its own (D-028). The stand-in's fleets still
            // show the 500 t aboard, so nothing moved; what did move is pinned further down.
            json!([]),
            "/fleets",
            "Done, but nothing was unloaded: the planet's storage is full. The cargo stays aboard.",
        ),
    ];
    for (fields, want, events, location, banner) in cases {
        w.api.reply("/commands", accepted(&events));
        let key = fresh_key(&mut b).await;
        let before = commands(&w).len();
        let mut sent = fields.clone();
        sent.push(("key", &key));
        let page = submit(&mut b, &sent).await;
        assert_eq!(page.status, 303, "{fields:?}: {}", page.body);
        assert_eq!(page.location.as_deref(), Some(location), "{fields:?}");
        let calls = commands(&w);
        assert_eq!(calls.len(), before + 1, "exactly one call: {fields:?}");
        let call = calls.last().unwrap();
        assert_eq!(call.idempotency_key.as_deref(), Some(key.as_str()));
        assert!(
            call.authorization
                .as_deref()
                .is_some_and(|a| a.starts_with("Bearer "))
        );
        assert_eq!(
            serde_json::from_str::<Value>(&call.body).unwrap(),
            want,
            "{fields:?}"
        );
        let shown = b.get(location).await;
        has(&shown.body, &format!("<p><strong>{banner}</strong></p>"));
        lacks(&shown.body, ALREADY_SUBMITTED);
        // One-shot: the next page doesn't repeat it.
        lacks(&b.get(location).await.body, banner);
    }
}

#[tokio::test]
async fn a_success_lists_its_events_and_unloading_nothing_says_so() {
    let (w, mut b) = world().await;
    w.api.reply(
        "/commands",
        accepted(&json!([{"type": "fleet_returned", "fleet_id": 23, "planet_id": 7}])),
    );
    let key = fresh_key(&mut b).await;
    submit(
        &mut b,
        &[
            ("action", "trigger_unload"),
            ("key", &key),
            ("return_to", "/fleets"),
            ("fleet_id", "21"),
            ("planet_id", "7"),
            ("aboard_ore_g", "0"),
            ("aboard_ingots_g", "500000000"),
        ],
    )
    .await;
    let page = b.get("/fleets").await.body;
    has(
        &page,
        "<p><strong>Done, but nothing was unloaded: the planet's storage is full. The cargo stays aboard.</strong></p><p>Fleet #23 is back at Planet #7.</p>",
    );
}

// ---------------------------------------------------------------------------------------------
// Outcomes (§5.1, §5.5)

fn build_fields(key: &str) -> Vec<(&str, &str)> {
    vec![
        ("action", "build_building"),
        ("key", key),
        ("return_to", "/planets/7"),
        ("planet_id", "7"),
        ("building_type", "drill"),
    ]
}

#[tokio::test]
async fn a_refusal_shows_its_copy_and_code_and_the_next_render_has_a_new_key() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", Reply::error(422, "no_free_slot"));
    let key = fresh_key(&mut b).await;
    let sent = submit(&mut b, &build_fields(&key)).await;
    assert_eq!(
        (sent.status, sent.location.as_deref()),
        (303, Some("/planets/7"))
    );
    let page = b.get("/planets/7").await.body;
    has(
        &page,
        "<p><strong>Not done: Every building slot on this planet is taken. Demolish or cancel a building to free one.</strong> (<code>no_free_slot</code>)</p>",
    );
    assert_ne!(
        key_of(&page),
        key,
        "a refused key would refuse forever: the page has a new one"
    );
    // The host recorded the refusal, so the same key again is "already submitted" (§5.2).
    submit(&mut b, &build_fields(&key)).await;
    let again = b.get("/planets/7").await.body;
    has(&again, "(<code>no_free_slot</code>)");
    has(&again, &format!("<p>{ALREADY_SUBMITTED}</p>"));
    assert_eq!(
        commands(&w).len(),
        2,
        "the web still sends: the API replays (T7)"
    );
}

#[tokio::test]
async fn a_refusal_names_the_form_s_target() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", Reply::error(422, "target_owned"));
    let key = fresh_key(&mut b).await;
    submit(
        &mut b,
        &[
            ("action", "send_fleet"),
            ("key", &key),
            ("return_to", "/fleets"),
            ("fleet_id", "24"),
            ("target_planet_id", "12"),
            ("mission", "colonize"),
        ],
    )
    .await;
    // A refusal goes back to the form's page, not to Fleets as a success would.
    has(
        &b.get("/fleets").await.body,
        "Not done: Planet #12 is already owned. Colonize needs an unowned planet; Move goes anywhere.",
    );
}

#[tokio::test]
async fn back_then_the_same_form_replays_and_a_different_form_conflicts() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    let key = fresh_key(&mut b).await;
    submit(&mut b, &build_fields(&key)).await;
    lacks(&b.get("/planets/7").await.body, ALREADY_SUBMITTED);
    // Back, then the same form: the API's replay, with §5.2's line after the outcome (D2).
    submit(&mut b, &build_fields(&key)).await;
    has(
        &b.get("/planets/7").await.body,
        &format!("It draws ingots until it's complete.</strong></p><p>{ALREADY_SUBMITTED}</p>"),
    );
    // Back, then a different form with the same key: D-037's conflict, with its copy.
    w.api
        .reply("/commands", Reply::error(422, "idempotency_key_conflict"));
    submit(
        &mut b,
        &[
            ("action", "queue_ship"),
            ("key", &key),
            ("return_to", "/planets/7"),
            ("shipyard_id", "16"),
            ("ship_type", "colony_ship"),
        ],
    )
    .await;
    let page = b.get("/planets/7").await.body;
    has(
        &page,
        "Not done: That form was already used for a different order, so this one wasn't sent. The page has fresh forms now: submit it again.</strong> (<code>idempotency_key_conflict</code>)",
    );
    // QA's F2: the key was answered, but "this is its result" would contradict the conflict.
    lacks(&page, ALREADY_SUBMITTED);
    let keys: Vec<_> = commands(&w)
        .iter()
        .map(|c| c.idempotency_key.clone().unwrap())
        .collect();
    assert_eq!(
        keys,
        [key.clone(), key.clone(), key],
        "every submit was this one key"
    );
}

#[tokio::test]
async fn a_400_is_a_bug_s_banner_and_doesn_t_mark_the_key() {
    let (w, mut b) = world().await;
    w.api
        .reply("/commands", Reply::error(400, "malformed_body"));
    let key = fresh_key(&mut b).await;
    submit(&mut b, &build_fields(&key)).await;
    has(
        &b.get("/planets/7").await.body,
        "Not done: The game didn't understand that order. This is a bug in the web client, not something you did; please report it.</strong> (<code>malformed_body</code>)",
    );
    w.api.reply("/commands", accepted(&json!([])));
    submit(&mut b, &build_fields(&key)).await;
    lacks(&b.get("/planets/7").await.body, ALREADY_SUBMITTED);
}

/// §5.5: a retryable failure is the retry page at the API's status, re-posting the identical
/// fields and key; the key isn't marked, so Try again shows the real outcome, plainly.
async fn retried(reply: Reply, status: u16, copy: &str, code: Option<&str>) {
    let (w, mut b) = world().await;
    w.api.reply("/commands", reply);
    let key = fresh_key(&mut b).await;
    let fields = build_fields(&key);
    let page = submit(&mut b, &fields).await;
    assert_eq!(page.status, status, "{}", page.body);
    has(&page.body, "<h1>Not sent yet</h1>");
    has(&page.body, copy);
    match code {
        Some(c) => has(&page.body, &format!("(<code>{c}</code>)")),
        None => lacks(&page.body, "<code>"),
    }
    has(&page.body, r#"<a href="/planets/7">Back to Planet #7</a>"#);
    let again = hidden_fields(&page.body);
    let want: Vec<(String, String)> = fields
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    assert_eq!(again, want, "the identical fields, the same key");
    assert_eq!(commands(&w).len(), 1);
    // Try again, now answered: Done, and no "already submitted".
    w.api.reply("/commands", accepted(&json!([])));
    let pairs: Vec<(&str, &str)> = again
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(submit(&mut b, &pairs).await.status, 303);
    let done = b.get("/planets/7").await.body;
    has(&done, "Done: construction of a Drill started on Planet #7.");
    lacks(&done, ALREADY_SUBMITTED);
    let calls = commands(&w);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].idempotency_key.as_deref(), Some(key.as_str()));
    assert_eq!(calls[0].body, calls[1].body);
}

#[tokio::test]
async fn a_rate_limit_is_the_retry_page_at_429_with_retry_after() {
    retried(
        Reply::error(429, "rate_limited").header("retry-after", "4"),
        429,
        "Too many requests at once. Try again in 4 seconds; your order wasn't sent.",
        Some("rate_limited"),
    )
    .await;
    // The header itself is passed on.
    let (w, mut b) = world().await;
    w.api.reply(
        "/commands",
        Reply::error(503, "busy").header("retry-after", "1"),
    );
    let key = fresh_key(&mut b).await;
    let req = b
        .http
        .post(format!("{}/act", b.web))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("{}={}", voidforge_web::routes::COOKIE_NAME, b.cookie.clone().unwrap()))
        .body(format!("action=build_building&key={key}&return_to=%2Fplanets%2F7&planet_id=7&building_type=drill"));
    let r = req.send().await.unwrap();
    assert_eq!(r.status().as_u16(), 503);
    assert_eq!(r.headers()["retry-after"], "1");
}

#[tokio::test]
async fn busy_unavailable_and_internal_are_retry_pages_at_their_status() {
    retried(
        Reply::error(503, "busy"),
        503,
        "The game is busy. Try again in a moment; your order wasn't sent.",
        Some("busy"),
    )
    .await;
    retried(
        Reply::error(503, "unavailable"),
        503,
        "The game is starting up or catching up. Try again in a moment; your order wasn't sent.",
        Some("unavailable"),
    )
    .await;
    retried(Reply::error(500, "internal"), 500, "Something went wrong on the game server. Your order may or may not have gone through. Trying again is safe: it won't run twice.", Some("internal")).await;
    retried(Reply::raw(StatusCode::IM_A_TEAPOT, "{}"), 502, "The game server answered something unexpected, so your order may or may not have gone through.", None).await;
}

#[tokio::test]
async fn an_unreachable_api_is_the_retry_page_at_502() {
    let w = World::start_with(
        Options {
            api_timeout: Duration::from_millis(800),
            ..Options::default()
        },
        |_| {},
    )
    .await;
    reply(&w, "/players/me/fleets", &my_fleets());
    let mut b = logged_in(&w).await;
    w.api.reply("/commands", accepted(&json!([])).delay(1_500));
    let key = fresh_key(&mut b).await;
    let page = submit(&mut b, &build_fields(&key)).await;
    assert_eq!(page.status, 502, "{}", page.body);
    has(&page.body, UNREACHABLE_COMMAND);
    assert_eq!(
        hidden_fields(&page.body)
            .iter()
            .find(|(k, _)| k == "key")
            .map(|(_, v)| v.as_str()),
        Some(key.as_str())
    );
}

#[tokio::test]
async fn a_401_ends_the_session_and_the_order_is_never_resent() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", Reply::error(401, "invalid_token"));
    let key = fresh_key(&mut b).await;
    let sent = submit(&mut b, &build_fields(&key)).await;
    assert_eq!(sent.status, 303);
    assert_eq!(sent.location.as_deref(), Some("/login?next=%2Fplanets%2F7"));
    // The session is over: the key is gone with it.
    w.api.reply("/commands", accepted(&json!([])));
    let callback = b.start_login("/login?next=%2Fplanets%2F7").await;
    let back = b.get(&callback).await;
    assert_eq!(back.location.as_deref(), Some("/planets/7"));
    let page = b.get("/planets/7").await.body;
    has(
        &page,
        &format!("<p><strong>{}</strong></p>", ORDER_NOT_SENT),
    );
    assert_eq!(commands(&w).len(), 1, "never auto-resubmitted (T5)");
    // The old key died with the session.
    submit(&mut b, &build_fields(&key)).await;
    has(&b.get("/planets/7").await.body, EXPIRED_FORM);
    assert_eq!(commands(&w).len(), 1);
}

#[tokio::test]
async fn an_expired_login_sends_nothing_and_says_so() {
    let w = World::start().await;
    reply(&w, "/planets/7", &home_planet(common::PLAYER));
    reply(&w, "/players/me/fleets", &my_fleets());
    w.behave(|b| b.claims = json!({ "exp": now() + 1 }));
    let mut b = logged_in(&w).await;
    let key = key_of(&b.get("/planets/7").await.body);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    w.api.reply("/commands", accepted(&json!([])));
    let page = submit(&mut b, &build_fields(&key)).await;
    assert_eq!(page.status, 200);
    has(&page.body, ACTION_EXPIRED);
    has(&page.body, r#"href="/login?next=%2Fplanets%2F7""#);
    assert!(
        commands(&w).is_empty(),
        "no POST /commands on the expired path (T5)"
    );
    // Logging in again returns to the page, says so once, and sends nothing by itself.
    w.behave(|b| b.claims = json!({}));
    let callback = b.start_login("/login?next=%2Fplanets%2F7").await;
    assert_eq!(
        b.get(&callback).await.location.as_deref(),
        Some("/planets/7")
    );
    has(&b.get("/planets/7").await.body, RENEWED);
    assert!(commands(&w).is_empty());
}

#[tokio::test]
async fn a_form_without_a_login_sends_nothing() {
    let w = World::start().await;
    let mut b = w.browser();
    let page = submit(
        &mut b,
        &[
            ("action", "build_building"),
            ("key", "AAAAAAAAAAAAAAAAAAAAAA"),
            ("return_to", "/planets/7"),
            ("planet_id", "7"),
            ("building_type", "drill"),
        ],
    )
    .await;
    assert_eq!(page.status, 200);
    has(&page.body, NOT_LOGGED_IN);
    has(&page.body, r#"href="/login?next=%2Fplanets%2F7""#);
    assert!(commands(&w).is_empty());
}

// ---------------------------------------------------------------------------------------------
// The key (Q3, T1-T3)

#[tokio::test]
async fn an_unissued_missing_or_foreign_key_is_refused_with_no_call() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    fresh_key(&mut b).await;
    let made_up = "AAAAAAAAAAAAAAAAAAAAAA";
    for fields in [
        build_fields(made_up),
        build_fields(""),
        vec![
            ("action", "build_building"),
            ("return_to", "/planets/7"),
            ("planet_id", "7"),
            ("building_type", "drill"),
        ],
    ] {
        let page = submit(&mut b, &fields).await;
        assert_eq!(
            (page.status, page.location.as_deref()),
            (303, Some("/planets/7"))
        );
        has(&b.get("/planets/7").await.body, EXPIRED_FORM);
    }
    // Another session's key: a forger's form, or another browser's.
    let mut other = logged_in(&w).await;
    let theirs = fresh_key(&mut other).await;
    submit(&mut b, &build_fields(&theirs)).await;
    has(&b.get("/planets/7").await.body, EXPIRED_FORM);
    assert!(commands(&w).is_empty(), "no call for any of them");
    // Their key still works for them.
    submit(&mut other, &build_fields(&theirs)).await;
    assert_eq!(commands(&w).len(), 1);
}

#[tokio::test]
async fn the_65th_render_drops_the_first_key() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    let first = fresh_key(&mut b).await;
    let second = fresh_key(&mut b).await;
    for _ in 2..MAX_FORM_KEYS {
        fresh_key(&mut b).await;
    }
    // 64 renders: the first still sends.
    submit(&mut b, &build_fields(&first)).await;
    assert_eq!(commands(&w).len(), 1);
    // The 65th drops it, and only it: the second is now the oldest kept.
    fresh_key(&mut b).await;
    submit(&mut b, &build_fields(&second)).await;
    assert_eq!(commands(&w).len(), 2);
    submit(&mut b, &build_fields(&first)).await;
    assert_eq!(commands(&w).len(), 2, "an evicted key makes no call");
    has(&b.get("/planets/7").await.body, EXPIRED_FORM);
}

#[tokio::test]
async fn two_renders_never_share_a_key() {
    let (_w, mut b) = world().await;
    let mut keys = Vec::new();
    for _ in 0..8 {
        keys.push(key_of(&b.get("/planets/7").await.body));
        keys.push(fresh_key(&mut b).await);
    }
    for k in &keys {
        assert_eq!(k.len(), 22);
        assert!(
            k.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "{k}"
        );
    }
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), keys.len());
}

// ---------------------------------------------------------------------------------------------
// The web's own checks (§6.3) and return_to (§5.1.5)

#[tokio::test]
async fn the_web_s_checks_refuse_before_any_call() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    for (fields, copy) in [
        (
            vec![("action", "build_building"), ("planet_id", "7")],
            "<strong>Choose a building type.</strong>",
        ),
        (
            vec![("action", "queue_ship"), ("shipyard_id", "16")],
            "<strong>Choose a ship type.</strong>",
        ),
        (
            vec![
                ("action", "send_fleet"),
                ("fleet_id", "24"),
                ("target_planet_id", "12"),
            ],
            "<strong>Choose a mission.</strong>",
        ),
        (
            vec![
                ("action", "send_fleet"),
                ("fleet_id", ""),
                ("target_planet_id", "12"),
                ("mission", "move"),
            ],
            "<strong>Choose a fleet.</strong>",
        ),
        (
            vec![
                ("action", "send_fleet"),
                ("fleet_id", "24"),
                ("target_planet_id", "#12"),
                ("mission", "move"),
            ],
            "<strong>Enter a planet number, like 12.</strong>",
        ),
        (
            vec![("action", "assemble_fleet"), ("planet_id", "7")],
            "<strong>Pick at least one ship.</strong>",
        ),
        (
            vec![
                ("action", "assemble_fleet"),
                ("planet_id", "7"),
                ("ship", "30"),
                ("ingots", "1,500"),
            ],
            "<strong>Enter cargo in tonnes, like 250 or 12.5.</strong>",
        ),
        (
            vec![
                ("action", "assemble_fleet"),
                ("planet_id", "7"),
                ("ship", "30"),
                ("ship", "30"),
            ],
            "(<code>duplicate_ship_ids</code>)",
        ),
        (
            vec![("action", "self_destruct")],
            "That form couldn't be read, so nothing was sent.",
        ),
    ] {
        let key = fresh_key(&mut b).await;
        let mut sent = fields.clone();
        sent.extend([("key", key.as_str()), ("return_to", "/planets/7")]);
        let page = submit(&mut b, &sent).await;
        assert_eq!(
            (page.status, page.location.as_deref()),
            (303, Some("/planets/7")),
            "{fields:?}"
        );
        has(&b.get("/planets/7").await.body, copy);
        // A refused form used nothing: its key still sends once it's right.
    }
    assert!(commands(&w).is_empty(), "no call for any check");
    let key = fresh_key(&mut b).await;
    submit(
        &mut b,
        &[
            ("action", "assemble_fleet"),
            ("planet_id", "7"),
            ("ship", "30"),
            ("ingots", "1,500"),
            ("key", &key),
        ],
    )
    .await;
    submit(
        &mut b,
        &[
            ("action", "assemble_fleet"),
            ("planet_id", "7"),
            ("ship", "30"),
            ("ingots", "1500"),
            ("key", &key),
        ],
    )
    .await;
    assert_eq!(commands(&w).len(), 1);
    lacks(&b.get("/fleets").await.body, ALREADY_SUBMITTED);
}

#[tokio::test]
async fn return_to_is_only_ever_a_local_path() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", Reply::error(422, "no_free_slot"));
    for to in [
        "//evil.example/x",
        "https://evil.example/",
        "/\\evil.example",
        "evil",
        "",
    ] {
        let key = fresh_key(&mut b).await;
        let page = submit(
            &mut b,
            &[
                ("action", "build_building"),
                ("key", &key),
                ("return_to", to),
                ("planet_id", "7"),
                ("building_type", "drill"),
            ],
        )
        .await;
        assert_eq!(page.location.as_deref(), Some("/"), "{to:?}");
    }
    // Even an expired form's refusal can't be steered off-site.
    let page = submit(
        &mut b,
        &build_fields("nope")
            .iter()
            .map(|(k, v)| {
                if *k == "return_to" {
                    (*k, "//evil.example")
                } else {
                    (*k, *v)
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;
    assert_eq!(page.location.as_deref(), Some("/"));
}

// ---------------------------------------------------------------------------------------------
// Confirm pages (§5.4)

#[tokio::test]
async fn the_confirm_pages_state_the_loss_and_carry_their_own_key() {
    let (w, mut b) = world().await;
    let planet = b.get("/planets/7").await.body;
    let planet_key = key_of(&planet);
    lacks(&planet, "key=");
    has(
        &planet,
        r#"<a href="/planets/7/cancel-construction/15">Cancel construction…</a>"#,
    );
    has(&planet, r#"<a href="/planets/7/demolish/12">Demolish…</a>"#);
    has(
        &planet,
        r#"<a href="/planets/7/cancel-build/20">Cancel…</a>"#,
    );
    let from = w.api.requests.lock().unwrap().len();
    for (path, h1, line, hidden, button) in [
        (
            "/planets/7/cancel-construction/15",
            "Cancel construction of Shipyard #15?",
            "Planet #7. The 504.0 t of ingots delivered so far are lost, and the slot frees now.",
            vec![
                ("action", "cancel_construction"),
                ("return_to", "/planets/7"),
                ("building_id", "15"),
                ("building_type", "shipyard"),
            ],
            "Cancel construction of Shipyard #15",
        ),
        (
            "/planets/7/demolish/12",
            "Demolish Drill #12?",
            "Planet #7. It stops working now, and nothing is refunded. The slot frees in 1 h, when demolition ends.",
            vec![
                ("action", "demolish_building"),
                ("return_to", "/planets/7"),
                ("building_id", "12"),
                ("building_type", "drill"),
            ],
            "Demolish Drill #12",
        ),
        (
            "/planets/7/cancel-build/20",
            "Cancel build #20 (Colony Ship) at Shipyard #16?",
            "Planet #7. The 240.0 t of ingots delivered so far are lost.",
            vec![
                ("action", "cancel_ship_build"),
                ("return_to", "/planets/7"),
                ("ship_build_id", "20"),
            ],
            "Cancel build #20",
        ),
        (
            "/fleets/22/turn-back",
            "Turn Fleet #22 back?",
            "It has flown 1 min 2 s, so it returns to Planet #7 in about that long, and can't be turned again. Its mission (Colonize Planet #12) is abandoned.",
            vec![
                ("action", "recall_fleet"),
                ("return_to", "/fleets"),
                ("fleet_id", "22"),
                ("origin", "7"),
            ],
            "Turn Fleet #22 back",
        ),
    ] {
        let page = b.get(path).await;
        assert_eq!(page.status, 200, "{path}: {}", page.body);
        has(&page.body, &format!("<h1>{h1}</h1><p>{line}</p>"));
        has(
            &page.body,
            &format!(r#"<button type="submit">{button}</button>"#),
        );
        let back = if path.starts_with("/fleets") {
            "/fleets"
        } else {
            "/planets/7"
        };
        let keep = if path.starts_with("/fleets") {
            "Keep flying"
        } else {
            "Keep it"
        };
        has(&page.body, &format!(r#"<a href="{back}">{keep}</a>"#));
        let key = key_of(&page.body);
        assert_ne!(key, planet_key, "the confirm page issues its own key");
        let mut want: Vec<(String, String)> = hidden
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        want.insert(1, ("key".to_owned(), key.clone()));
        assert_eq!(hidden_fields(&page.body), want, "{path}");
        // Confirming sends it, with the confirm page's key.
        w.api.reply("/commands", accepted(&json!([])));
        let pairs: Vec<(String, String)> = hidden_fields(&page.body);
        let pairs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(submit(&mut b, &pairs).await.location.as_deref(), Some(back));
        assert_eq!(
            commands(&w).last().unwrap().idempotency_key.as_deref(),
            Some(key.as_str())
        );
    }
    // The pages only read: one command per confirm, from the POSTs.
    let posts = w.api.requests.lock().unwrap()[from..]
        .iter()
        .filter(|r| r.method == axum::http::Method::POST)
        .count();
    assert_eq!(posts, 4);
}

#[tokio::test]
async fn a_confirm_page_reads_what_it_states() {
    let (w, mut b) = world().await;
    let from = w.api.requests.lock().unwrap().len();
    b.get("/planets/7/demolish/12").await;
    let mut reads: Vec<String> = w.api.requests.lock().unwrap()[from..]
        .iter()
        .map(|r| r.uri.clone())
        .collect();
    reads.sort();
    assert_eq!(reads, ["/planets/7", "/players/me", "/rules"]);
    let from = w.api.requests.lock().unwrap().len();
    b.get("/fleets/22/turn-back").await;
    let mut reads: Vec<String> = w.api.requests.lock().unwrap()[from..]
        .iter()
        .map(|r| r.uri.clone())
        .collect();
    reads.sort();
    assert_eq!(reads, ["/players/me", "/players/me/fleets", "/rules"]);
    // Without /rules, Demolish's line has no time (§5.7).
    *w.api.rules.lock().unwrap() = Reply::error(500, "internal");
    has(
        &b.get("/planets/7/demolish/12").await.body,
        "<p>Planet #7. It stops working now, and nothing is refunded. The slot frees when demolition ends.</p>",
    );
}

#[tokio::test]
async fn a_confirm_page_for_something_gone_shows_why_and_no_form() {
    let (w, mut b) = world().await;
    for (path, copy) in [
        (
            "/planets/7/cancel-construction/12",
            "That building isn't under construction any more. It may have just been completed. (<code>not_under_construction</code>)",
        ),
        (
            "/planets/7/cancel-construction/99",
            "That building no longer exists. It may have just been demolished or cancelled. (<code>unknown_building</code>)",
        ),
        (
            "/planets/7/demolish/16",
            "That Shipyard still has ships building or waiting. Cancel them before demolishing it. (<code>shipyard_has_builds</code>)",
        ),
        (
            "/planets/7/demolish/13",
            "That building isn't working: it's under construction or being demolished. (<code>not_operational</code>)",
        ),
        ("/planets/7/demolish/15", "(<code>not_operational</code>)"),
        (
            "/planets/7/cancel-build/99",
            "That ship build no longer exists. It may have just finished or been cancelled. (<code>unknown_ship_build</code>)",
        ),
        (
            "/fleets/21/turn-back",
            "That fleet isn't flying a mission, so there's nothing to turn back. It may have just arrived. (<code>fleet_not_in_transit</code>)",
        ),
        (
            "/fleets/23/turn-back",
            "That fleet has already turned back once, and it can't again. (<code>already_recalled</code>)",
        ),
        (
            "/fleets/99/turn-back",
            "That fleet no longer exists. It may have been disbanded, or used its last ship to colonize. (<code>unknown_fleet</code>)",
        ),
    ] {
        let page = b.get(path).await;
        assert_eq!(page.status, 200, "{path}");
        has(&page.body, copy);
        let back = if path.starts_with("/fleets") {
            r#"<a href="/fleets">Back to your fleets</a>"#
        } else {
            r#"<a href="/planets/7">Back to Planet #7</a>"#
        };
        has(&page.body, back);
        lacks(&page.body, r#"action="/act""#);
    }
    // Someone else's planet: no form either.
    reply(
        &w,
        "/planets/7",
        &home_planet("01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f"),
    );
    let page = b.get("/planets/7/demolish/12").await.body;
    has(
        &page,
        "Planet #7 isn't yours. You can build, and load cargo, only on your own planets. (<code>not_owner</code>)",
    );
    lacks(&page, r#"action="/act""#);
    assert!(commands(&w).is_empty());
}

#[tokio::test]
async fn a_confirm_path_that_isn_t_numbers_is_not_found_with_no_call() {
    let (w, mut b) = world().await;
    let from = w.api.requests.lock().unwrap().len();
    for path in [
        "/planets/x/demolish/12",
        "/planets/7/demolish/twelve",
        "/fleets/x/turn-back",
    ] {
        assert_eq!(b.get(path).await.status, 404, "{path}");
    }
    assert_eq!(w.api.requests.lock().unwrap().len(), from);
}

#[tokio::test]
async fn logged_out_a_confirm_page_is_the_logged_out_home_there() {
    let w = World::start().await;
    let page = w.browser().get("/planets/7/demolish/12").await;
    assert_eq!(page.status, 200);
    has(
        &page.body,
        r#"href="/login?next=%2Fplanets%2F7%2Fdemolish%2F12""#,
    );
    lacks(&page.body, r#"action="/act""#);
}

#[tokio::test]
async fn fleets_target_pre_fills_every_send_form() {
    let (_w, mut b) = world().await;
    let body = b.get("/fleets?target=12").await.body;
    let filled = r#"<input type="text" name="target_planet_id" size="8" value="12">"#;
    // Fleets 21 and 24 are stationed: one Send form each, both pre-filled.
    assert_eq!(body.matches(filled).count(), 2, "{body}");
    // The Send forms return to the page as it was asked for.
    has(&body, r#"name="return_to" value="/fleets?target=12""#);
    for odd in ["/fleets?target=x", "/fleets?target=-1", "/fleets"] {
        let body = b.get(odd).await.body;
        assert_eq!(
            body.matches(r#"name="target_planet_id" size="8" value="">"#)
                .count(),
            2,
            "{odd}"
        );
    }
}

fn max_age(set_cookie: &str) -> u64 {
    set_cookie
        .to_ascii_lowercase()
        .split(';')
        .find_map(|a| a.trim().strip_prefix("max-age=").map(str::to_owned))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no max-age: {set_cookie}"))
}

/// Every step that changes the session (a render's key, a confirm page's key, a submit's banner
/// and answered mark, the page that takes the banner) keeps the login's lifetime: a save
/// otherwise cuts it to the anonymous 10 minutes (D-057's tower-sessions lesson).
#[tokio::test]
async fn issuing_keys_and_banners_keeps_the_login_s_lifetime() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    let want = 3600 + voidforge_web::routes::EXPIRED_GRACE_SECS;
    let check = |b: &Browser, step: &str, from: usize| {
        let set: Vec<&String> = b.set_cookies[from..].iter().collect();
        assert!(
            !set.is_empty(),
            "{step}: the session was saved, with its cookie"
        );
        for c in set {
            assert!((want - 5..=want).contains(&max_age(c)), "{step}: {c}");
        }
    };
    let from = b.set_cookies.len();
    let key = key_of(&b.get("/planets/7").await.body);
    check(&b, "a planet render", from);
    let from = b.set_cookies.len();
    b.get("/planets/7/demolish/12").await;
    check(&b, "a confirm page", from);
    let from = b.set_cookies.len();
    submit(&mut b, &build_fields(&key)).await;
    check(&b, "a submit", from);
    let from = b.set_cookies.len();
    b.get("/fleets").await;
    check(&b, "the page taking the banner", from);
    // A refusal before any call changes the session only by its banner: the table's survivor
    // on 598cf30 (a 200 or 422 re-asserts the expiry when it marks the key; these don't).
    let key = fresh_key(&mut b).await;
    let from = b.set_cookies.len();
    submit(
        &mut b,
        &[
            ("action", "build_building"),
            ("key", &key),
            ("return_to", "/planets/7"),
            ("planet_id", "7"),
        ],
    )
    .await;
    check(&b, "a submit the web's checks refused", from);
    let from = b.set_cookies.len();
    submit(&mut b, &build_fields("AAAAAAAAAAAAAAAAAAAAAA")).await;
    check(&b, "an expired form", from);
    assert!(commands(&w).len() == 1, "neither refusal called");
}

/// QA's F1 and the lead's T9: the core answers Trigger unload with no event, so the banner says
/// what moved from the fleet read after the 200; a failed read is the neutral line.
#[tokio::test]
async fn an_unload_says_what_moved_from_the_fleet_after_it() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    let page = b.get("/fleets").await.body;
    let fields = act_form_fields(&page, "trigger_unload");
    assert!(
        fields.contains(&("aboard_ingots_g".to_owned(), "500000000".to_owned())),
        "{fields:?}"
    );
    // After it, 20 t is still aboard: 480 t moved, and the rest is named.
    let mut after = my_fleets();
    after["fleets"][0]["cargo"]["iron_ingot_g"] = json!(20_000_000);
    // Another stationed fleet (24, empty) comes first in the read, so only the unloaded fleet's
    // own cargo gives these words (the table's survivor on 1d3a6a4).
    after["fleets"].as_array_mut().unwrap().rotate_right(1);
    reply(&w, "/players/me/fleets", &after);
    let from = w.api.requests_to("/players/me/fleets").len();
    let pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    submit(&mut b, &pairs).await;
    assert_eq!(
        w.api.requests_to("/players/me/fleets").len(),
        from + 1,
        "one read after the 200"
    );
    let shown = b.get("/fleets").await.body;
    has(
        &shown,
        "<p><strong>Done: unloaded 480.0 t of ingots into Planet #7. 20.0 t of ingots stays aboard: the storage is full.</strong></p>",
    );
    lacks(&shown, "nothing was unloaded");
    // Everything moved.
    reply(&w, "/players/me/fleets", &my_fleets());
    let page = b.get("/fleets").await.body;
    let fields = act_form_fields(&page, "trigger_unload");
    let mut after = my_fleets();
    after["fleets"][0]["cargo"]["iron_ingot_g"] = json!(0);
    reply(&w, "/players/me/fleets", &after);
    let pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    submit(&mut b, &pairs).await;
    has(
        &b.get("/fleets").await.body,
        "<p><strong>Done: unloaded 500.0 t of ingots into Planet #7.</strong></p>",
    );
    // The read after it fails: the neutral line, never "nothing was unloaded".
    reply(&w, "/players/me/fleets", &my_fleets());
    let page = b.get("/fleets").await.body;
    let fields = act_form_fields(&page, "trigger_unload");
    w.api.reply("/players/me/fleets", Reply::error(503, "busy"));
    let pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let sent = submit(&mut b, &pairs).await;
    assert_eq!(sent.status, 303);
    reply(&w, "/players/me/fleets", &my_fleets());
    has(
        &b.get("/fleets").await.body,
        "<p><strong>Done: Fleet #21 unloaded what fit into Planet #7's storage.</strong></p>",
    );
    assert_eq!(
        commands(&w).len(),
        3,
        "one command per submit, the read never re-sends"
    );
}

/// The hidden fields of a page's form for `action`, as a browser posts them.
fn act_form_fields(page: &str, action: &str) -> Vec<(String, String)> {
    page.split(r#"<form method="post" action="/act">"#)
        .skip(1)
        .map(|f| hidden_fields(&f[..f.find("</form>").unwrap()]))
        .find(|fields| fields.iter().any(|(k, v)| k == "action" && v == action))
        .unwrap_or_else(|| panic!("no {action} form in:\n{page}"))
}

/// Review-r2's delta on 241e42a: an answered Unload form submitted again gets the first answer
/// back (D-037), which the fleet's cargo now can't describe; the neutral line and "already
/// submitted", and no read.
#[tokio::test]
async fn a_replayed_unload_is_neutral_and_already_submitted() {
    let (w, mut b) = world().await;
    w.api.reply("/commands", accepted(&json!([])));
    let page = b.get("/fleets").await.body;
    let fields = act_form_fields(&page, "trigger_unload");
    let pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut after = my_fleets();
    after["fleets"][0]["cargo"]["iron_ingot_g"] = json!(0);
    reply(&w, "/players/me/fleets", &after);
    submit(&mut b, &pairs).await;
    has(
        &b.get("/fleets").await.body,
        "<p><strong>Done: unloaded 500.0 t of ingots into Planet #7.</strong></p>",
    );
    // The fleet carries the same cargo again (say it was reloaded), then the old form comes back.
    reply(&w, "/players/me/fleets", &my_fleets());
    let reads = w.api.requests_to("/players/me/fleets").len();
    submit(&mut b, &pairs).await;
    assert_eq!(
        w.api.requests_to("/players/me/fleets").len(),
        reads,
        "no read for a replay"
    );
    let shown = b.get("/fleets").await.body;
    has(
        &shown,
        &format!(
            "<p><strong>Done: Fleet #21 unloaded what fit into Planet #7's storage.</strong></p><p>{ALREADY_SUBMITTED}</p>"
        ),
    );
    lacks(&shown, "nothing was unloaded");
}
