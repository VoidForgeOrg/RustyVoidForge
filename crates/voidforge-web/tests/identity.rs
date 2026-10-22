//! `--identity-token` (P5.2, D-065): with it, every response carries the walk's token in
//! `x-voidforge-identity`, a page's, a redirect's and a 404's alike; without it, none does.

use axum::Router;
use axum::http::HeaderValue;
use axum::response::Redirect;
use axum::routing::get;
use voidforge_web::routes::{IDENTITY_HEADER, with_identity};

/// Serves `app` on a free port; its base URL.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn app() -> Router {
    Router::new()
        .route("/", get(|| async { "home" }))
        .route("/login", get(|| async { Redirect::to("/elsewhere") }))
}

#[tokio::test]
async fn every_response_carries_the_token() {
    let base = serve(with_identity(app(), HeaderValue::from_static("walk-1234"))).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for (path, status) in [("/", 200), ("/login", 303), ("/missing", 404)] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{path}");
        assert_eq!(
            response.headers().get(IDENTITY_HEADER).unwrap(),
            "walk-1234",
            "{path}"
        );
    }
}

#[tokio::test]
async fn without_the_flag_no_response_carries_one() {
    let base = serve(app()).await;
    let response = reqwest::get(format!("{base}/")).await.unwrap();
    assert!(response.headers().get(IDENTITY_HEADER).is_none());
}
