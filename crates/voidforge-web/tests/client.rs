//! The API client by effect against the scripted stand-in (P4.2, D-059): each route decodes its
//! view and sends the token only when it's authenticated, and every way a call can end is one
//! `ApiError` variant, forced here one by one.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{Reply, api_stand_in, player_view};
use serde_json::{Value, json};
use voidforge_api_types::command::{BuildBuilding, CommandRequest, IdempotencyKey};
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::wire::{BuildingType, PlanetId, Position};
use voidforge_web::api::{Api, ApiError, MAX_BODY_BYTES, MAX_RETRY_AFTER_SECS, Unreachable};
use voidforge_web::config::Secret;

const TOKEN: &str = "eyJ.the-id-token-itself.sig";

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn token() -> Secret {
    Secret::new(TOKEN.to_owned())
}

fn client(base: &str) -> Api {
    Api::new(base.to_owned()).unwrap()
}

fn key() -> IdempotencyKey {
    IdempotencyKey::parse(Some("form-key-1")).unwrap()
}

fn build() -> CommandRequest {
    CommandRequest::BuildBuilding(BuildBuilding {
        planet_id: PlanetId(4),
        building_type: BuildingType::Drill,
    })
}

/// `/players/me` answers `reply` (through the stand-in's `answer`, which is JSON only).
fn me_answers(api: &common::Api, status: u16, body: Value) {
    *api.answer.lock().unwrap() = (StatusCode::from_u16(status).unwrap(), body);
}

#[tokio::test]
async fn every_route_decodes_its_view_and_only_authenticated_ones_send_the_token() {
    let (stand_in, base) = api_stand_in().await;
    stand_in.reply(
        "/players/me/fleets",
        Reply::json(StatusCode::OK, &fixture("fleets_bob")),
    );
    stand_in.reply(
        "/planets/1",
        Reply::json(StatusCode::OK, &fixture("planet_view_alice_home")),
    );
    stand_in.reply(
        "/planets",
        Reply::json(StatusCode::OK, &fixture("planets_near")),
    );
    stand_in.reply(
        "/leaderboard",
        Reply::json(
            StatusCode::OK,
            &json!({"as_of": "2026-10-20T12:00:00.000Z", "entries": []}),
        ),
    );
    stand_in.reply(
        "/commands",
        Reply::json(StatusCode::OK, &fixture("command_accepted")),
    );
    let api = client(&base);
    let t = token();

    assert_eq!(api.me(&t).await.unwrap().score, 42);
    api.my_fleets(&t).await.unwrap();
    assert_eq!(
        api.planet(&t, PlanetId(1)).await.unwrap().planet_id,
        PlanetId(1)
    );
    api.planets_near(
        &t,
        Position {
            x: -150,
            y: 20,
            z: 0,
        },
        100_000,
    )
    .await
    .unwrap();
    api.leaderboard(&t).await.unwrap();
    api.command(&t, &key(), &build()).await.unwrap();
    assert_eq!(api.rules().await.unwrap().name, "v0");

    let requests = stand_in.requests.lock().unwrap().clone();
    let bearer = format!("Bearer {TOKEN}");
    let by_uri: Vec<(String, String, Option<String>)> = requests
        .iter()
        .map(|r| (r.method.to_string(), r.uri.clone(), r.authorization.clone()))
        .collect();
    assert_eq!(
        by_uri,
        [
            ("GET", "/players/me", Some(&bearer)),
            ("GET", "/players/me/fleets", Some(&bearer)),
            ("GET", "/planets/1", Some(&bearer)),
            (
                "GET",
                "/planets?near=-150,20,0&radius=100000",
                Some(&bearer)
            ),
            ("GET", "/leaderboard", Some(&bearer)),
            ("POST", "/commands", Some(&bearer)),
            ("GET", "/rules", None),
        ]
        .map(|(m, u, a)| (m.to_owned(), u.to_owned(), a.cloned()))
    );
    let command = &requests[5];
    assert_eq!(command.idempotency_key.as_deref(), Some("form-key-1"));
    assert_eq!(
        serde_json::from_str::<Value>(&command.body).unwrap(),
        json!({"type": "build_building", "payload": {"planet_id": 4, "building_type": "drill"}})
    );
}

#[tokio::test]
async fn every_code_at_its_status_is_refused_with_that_code() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    for code in ErrorCode::ALL {
        let status = code.status();
        me_answers(
            &stand_in,
            status,
            json!({"code": code.as_str(), "message": "m"}),
        );
        stand_in.reply("/commands", Reply::error(status, code.as_str()));
        let want = ApiError::Refused {
            code,
            status,
            retry_after: None,
        };
        assert_eq!(api.me(&token()).await, Err(want.clone()), "{code:?}");
        assert_eq!(
            api.command(&token(), &key(), &build()).await,
            Err(want),
            "{code:?}"
        );
    }
}

#[tokio::test]
async fn unknown_planet_at_404_is_a_refusal_only_from_one_planet() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    stand_in.reply("/planets/9", Reply::error(404, "unknown_planet"));
    stand_in.reply("/commands", Reply::error(404, "unknown_planet"));
    assert_eq!(
        api.planet(&token(), PlanetId(9)).await,
        Err(ApiError::Refused {
            code: ErrorCode::UnknownPlanet,
            status: 404,
            retry_after: None
        })
    );
    assert_eq!(
        api.command(&token(), &key(), &build()).await,
        Err(ApiError::UnexpectedStatus(404))
    );
}

#[tokio::test]
async fn a_code_at_a_status_it_isnt_sent_with_is_unexpected() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    for (status, code) in [
        (400, "busy"),
        (500, "invalid_token"),
        (422, "rate_limited"),
        (503, "internal"),
    ] {
        me_answers(&stand_in, status, json!({"code": code, "message": "m"}));
        assert_eq!(
            api.me(&token()).await,
            Err(ApiError::UnexpectedStatus(status)),
            "{code} at {status}"
        );
    }
}

#[tokio::test]
async fn retry_after_is_kept_on_429_and_503_only_and_bounded() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    for (status, code) in [(429, "rate_limited"), (503, "busy"), (503, "unavailable")] {
        for (header, want) in [
            (Some("1"), Some(1)),
            (Some("7"), Some(7)),
            (None, None),
            (Some("abc"), None),
            (Some("-3"), None),
            (Some("99999"), Some(MAX_RETRY_AFTER_SECS)),
        ] {
            let mut reply = Reply::error(status, code);
            if let Some(h) = header {
                reply = reply.header("retry-after", h);
            }
            stand_in.reply("/commands", reply);
            assert_eq!(
                api.command(&token(), &key(), &build()).await,
                Err(ApiError::Refused {
                    code: ErrorCode::ALL
                        .into_iter()
                        .find(|c| c.as_str() == code)
                        .unwrap(),
                    status,
                    retry_after: want
                }),
                "{code} with {header:?}"
            );
        }
    }
    stand_in.reply(
        "/commands",
        Reply::error(422, "no_free_slot").header("retry-after", "5"),
    );
    assert_eq!(
        api.command(&token(), &key(), &build()).await,
        Err(ApiError::Refused {
            code: ErrorCode::NoFreeSlot,
            status: 422,
            retry_after: None
        })
    );
}

#[tokio::test]
async fn nothing_listening_is_unreachable_and_reported_within_the_connect_bound() {
    let closed = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let api = client(&format!("http://{closed}"));
    let started = std::time::Instant::now();
    assert_eq!(
        api.me(&token()).await,
        Err(ApiError::Unreachable(Unreachable::Connect))
    );
    assert_eq!(
        api.rules().await,
        Err(ApiError::Unreachable(Unreachable::Connect))
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn an_answer_past_the_time_bound_is_a_timeout() {
    let (stand_in, base) = api_stand_in().await;
    let api =
        Api::with_timeouts(base, Duration::from_millis(300), Duration::from_millis(300)).unwrap();
    stand_in.reply(
        "/commands",
        Reply::json(StatusCode::OK, &fixture("command_accepted")).delay(1_500),
    );
    let started = std::time::Instant::now();
    assert_eq!(
        api.command(&token(), &key(), &build()).await,
        Err(ApiError::Unreachable(Unreachable::Timeout))
    );
    assert!(started.elapsed() < Duration::from_millis(1_200));
    // The same answer within the bound is the view: the bound, not the route, refused it.
    stand_in.reply(
        "/commands",
        Reply::json(StatusCode::OK, &fixture("command_accepted")).delay(50),
    );
    assert!(api.command(&token(), &key(), &build()).await.is_ok());
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    // The target answers a valid view, so a client that followed would succeed.
    stand_in.reply(
        "/players/me/fleets",
        Reply::raw(StatusCode::FOUND, "").header("location", &format!("{base}/fleets-moved")),
    );
    stand_in.reply(
        "/fleets-moved",
        Reply::json(StatusCode::OK, &fixture("fleets_bob")),
    );
    assert_eq!(
        api.my_fleets(&token()).await,
        Err(ApiError::UnexpectedStatus(302))
    );
    assert!(stand_in.requests_to("/fleets-moved").is_empty());
}

#[tokio::test]
async fn a_status_no_route_sends_is_unexpected_whatever_its_body() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    for (status, body) in [
        (204, ""),
        (201, r#"{"seq": 1}"#),
        (418, "<html>teapot</html>"),
        (418, r#"{"code": "busy", "message": "m"}"#),
        (502, "<html>bad gateway</html>"),
    ] {
        stand_in.reply(
            "/commands",
            Reply::raw(StatusCode::from_u16(status).unwrap(), body),
        );
        assert_eq!(
            api.command(&token(), &key(), &build()).await,
            Err(ApiError::UnexpectedStatus(status)),
            "{status} {body}"
        );
    }
    // 404 is sent only for unknown_planet, from one planet: from `/players/me` it's unexpected.
    me_answers(&stand_in, 404, json!({"code": "not_found", "message": "m"}));
    assert_eq!(
        api.me(&token()).await,
        Err(ApiError::Refused {
            code: ErrorCode::NotFound,
            status: 404,
            retry_after: None
        })
    );
}

#[tokio::test]
async fn a_body_that_isnt_the_contract_is_undecodable() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    // A 200 that isn't the route's view: another view, an unknown field, HTML.
    for body in [fixture("fleets_bob"), {
        let mut v = player_view(1);
        v["extra"] = json!(1);
        v
    }] {
        me_answers(&stand_in, 200, body);
        assert_eq!(
            api.me(&token()).await,
            Err(ApiError::Undecodable { status: 200 })
        );
    }
    *stand_in.rules.lock().unwrap() = Reply::raw(StatusCode::OK, "<html>rules</html>");
    assert_eq!(
        api.rules().await,
        Err(ApiError::Undecodable { status: 200 })
    );
    // An error status whose body isn't the error body.
    for status in [400, 401, 422, 429, 500, 503] {
        stand_in.reply(
            "/commands",
            Reply::raw(StatusCode::from_u16(status).unwrap(), "<html>oops</html>"),
        );
        assert_eq!(
            api.command(&token(), &key(), &build()).await,
            Err(ApiError::Undecodable { status }),
            "{status}"
        );
    }
    // An error body naming a code the contract doesn't have.
    stand_in.reply("/commands", Reply::error(422, "no_such_code"));
    assert_eq!(
        api.command(&token(), &key(), &build()).await,
        Err(ApiError::Undecodable { status: 422 })
    );
}

/// A valid view padded with whitespace to `len` bytes: JSON allows it, so only the cap can refuse
/// it (a body that's also not a view would be refused anyway, and pin nothing).
fn padded_rules(len: usize) -> Vec<u8> {
    let mut body = common::rules_view("v0").to_string().into_bytes();
    body.resize(len, b' ');
    body
}

/// Answers one request with `body` in 1 MiB chunks and no declared length, from a raw server.
fn chunked_server(body: Vec<u8>) -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let (mut s, _) = l.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf).unwrap();
        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n";
        let mut send = || -> std::io::Result<()> {
            s.write_all(head.as_bytes())?;
            for chunk in body.chunks(1024 * 1024) {
                s.write_all(format!("{:x}\r\n", chunk.len()).as_bytes())?;
                s.write_all(chunk)?;
                s.write_all(b"\r\n")?;
            }
            s.write_all(b"0\r\n\r\n")
        };
        // Past the cap the client hangs up, so a failed write is the expected end there.
        let _ = send();
    });
    addr
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused_unread() {
    let (stand_in, base) = api_stand_in().await;
    let api = client(&base);
    // A valid view at the cap is read; one byte more, with its length declared, isn't.
    *stand_in.rules.lock().unwrap() = Reply {
        body: padded_rules(MAX_BODY_BYTES),
        ..Reply::raw(StatusCode::OK, "")
    };
    assert_eq!(api.rules().await.map(|r| r.name), Ok("v0".to_owned()));
    *stand_in.rules.lock().unwrap() = Reply {
        body: padded_rules(MAX_BODY_BYTES + 1),
        ..Reply::raw(StatusCode::OK, "")
    };
    assert_eq!(
        api.rules().await,
        Err(ApiError::Undecodable { status: 200 })
    );

    // The same without a declared length (chunked): the read stops at the cap.
    let at = chunked_server(padded_rules(MAX_BODY_BYTES));
    assert_eq!(
        client(&format!("http://{at}"))
            .rules()
            .await
            .map(|r| r.name),
        Ok("v0".to_owned())
    );
    let over = chunked_server(padded_rules(MAX_BODY_BYTES + 1));
    assert_eq!(
        client(&format!("http://{over}")).rules().await,
        Err(ApiError::Undecodable { status: 200 })
    );
}

/// A connect that hangs (a listener that never accepts, its backlog already full) is reported at
/// the connect bound, not the total: the bound is what reports a dead API in seconds.
#[tokio::test]
async fn a_connect_that_hangs_ends_at_the_connect_bound() {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(1).unwrap();
    let addr = listener.local_addr().unwrap();
    // Fill the backlog; these are never accepted.
    let mut held = Vec::new();
    for _ in 0..8 {
        if let Ok(Ok(s)) = tokio::time::timeout(
            Duration::from_millis(200),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        {
            held.push(s);
        }
    }
    let api = Api::with_timeouts(
        format!("http://{addr}"),
        Duration::from_secs(10),
        Duration::from_millis(300),
    )
    .unwrap();
    let started = std::time::Instant::now();
    // `me`, bounded only by the total: `rules` has its own 2 s bound, which would hide this one.
    let got = api.me(&token()).await;
    let took = started.elapsed();
    eprintln!("held {} connections; {got:?} after {took:?}", held.len());
    assert_eq!(got, Err(ApiError::Unreachable(Unreachable::Timeout)));
    assert!(took < Duration::from_secs(1), "{took:?}");
    drop((listener, held));
}
