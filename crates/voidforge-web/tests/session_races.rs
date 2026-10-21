//! Session races found by review-r2 on P4.4's 05781b6 (its probes, adopted as regression tests):
//! a logout is final, even against a render that loaded the session before it (the store's `save`
//! never recreates a deleted record), and a new login starts with no form keys (the callback
//! clears what `cycle_id` keeps). D-061.

mod common;

use axum::http::StatusCode;
use common::{Reply, World};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_render_started_before_logout_must_not_restore_the_old_session() {
    let w = World::start().await;
    w.api.reply(
        "/players/me/fleets",
        Reply::json(
            StatusCode::OK,
            &json!({"as_of":"2026-10-20T12:00:00.000Z", "valid_until":null, "fleets":[]}),
        )
        .delay(800),
    );
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    let cookie = format!(
        "{}={}",
        voidforge_web::routes::COOKIE_NAME,
        b.cookie.clone().unwrap()
    );
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let pending = tokio::spawn({
        let http = http.clone();
        let cookie = cookie.clone();
        let url = format!("{}/fleets", w.web);
        async move { http.get(url).header("cookie", cookie).send().await.unwrap() }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while w.api.requests_to("/players/me/fleets").is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the render loaded the old session and reached the delayed API read");
    let logout = http
        .post(format!("{}/logout", w.web))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 303);
    let before = http
        .get(format!("{}/", w.web))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !before.contains("Your empire"),
        "logout initially invalidated the old cookie"
    );
    let render = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(render.status(), 200);
    let after = http
        .get(format!("{}/", w.web))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    eprintln!(
        "old cookie: logged out before held render finished={}, logged in after it finished={}",
        !before.contains("Your empire"),
        after.contains("Your empire")
    );
    assert!(
        !after.contains("Your empire"),
        "a completed logout must remain effective after an older render completes"
    );
}

#[tokio::test]
async fn a_new_login_must_drop_prior_form_keys() {
    let w = World::start().await;
    w.api.reply(
        "/players/me/fleets",
        Reply::json(
            StatusCode::OK,
            &json!({"as_of":"2026-10-20T12:00:00.000Z", "valid_until":null, "fleets":[]}),
        ),
    );
    let mut b = w.browser();
    assert_eq!(b.login().await.status, 303);
    // A confirm page offers a real form, with its key, using a known owned planet fixture.
    w.api.reply("/planets/7", Reply::json(StatusCode::OK, &json!({
        "planet_id":7,"owner":common::PLAYER,"as_of":"2026-10-20T12:00:00.000Z","valid_until":null,
        "stocks":{"iron_ore":{"amount_g":0,"rate_g_per_h":0,"cap_g":1000000},"iron_ingot":{"amount_g":0,"rate_g_per_h":0,"cap_g":1000000}},
        "buildings":[],"ship_builds":[],"roster":[],"position":{"x":0,"y":0,"z":0},"slots":8
    })));
    let p = b.get("/planets/7").await;
    assert_eq!(p.status, 200);
    let marker = "name=\"key\" value=\"";
    let key = p
        .body
        .split(marker)
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let old_cookie = b.cookie.clone();
    assert_eq!(b.login().await.status, 303);
    assert_ne!(b.cookie, old_cookie, "login cycles the ID");
    w.api.reply(
        "/commands",
        Reply::json(
            StatusCode::OK,
            &json!({"seq":2,"game_time":"2026-10-20T12:00:00.000Z","events":[]}),
        ),
    );
    let request = b.http.post(format!("{}/act", b.web)).header("content-type","application/x-www-form-urlencoded").body(format!("key={key}&action=build_building&planet_id=7&building_type=drill&return_to=%2Fplanets%2F7"));
    let r = b.send(request).await;
    assert_eq!(r.status, 303);
    eprintln!(
        "old form key forwarded after new login: {}",
        w.api.requests_to("/commands").len()
    );
    assert!(
        w.api.requests_to("/commands").is_empty(),
        "D-061 says a new login drops the old form keys"
    );
}
