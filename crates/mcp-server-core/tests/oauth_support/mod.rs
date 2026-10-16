#![allow(dead_code)]
use axum::body::Body;
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use mcp_server_core::oauth::{AuthState, OAuthSettings, oauth_router};
use std::sync::Arc;
use tower::ServiceExt;

pub const PUBLIC_URL: &str = "https://mail.example.com";
pub const PASSWORD: &str = "correct horse battery staple";

pub fn state() -> Arc<AuthState> {
    state_with_static(None)
}

pub fn state_with_static(token: Option<&str>) -> Arc<AuthState> {
    AuthState::new(OAuthSettings {
        public_url: PUBLIC_URL.into(),
        signing_key: "0123456789abcdef0123456789abcdef".to_string().into(),
        admin_password: PASSWORD.to_string().into(),
        static_token: token.map(|t| t.to_string().into()),
    })
}

pub fn app(state: Arc<AuthState>) -> axum::Router {
    oauth_router(state)
}

pub async fn send(app: &axum::Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

pub async fn body_bytes(res: Response<Body>) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

pub async fn body_json(res: Response<Body>) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(res).await).unwrap()
}

pub async fn body_text(res: Response<Body>) -> String {
    String::from_utf8(body_bytes(res).await).unwrap()
}

pub fn json_post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn form_post(uri: &str, pairs: &[(&str, &str)]) -> Request<Body> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [10, 0, 0, 1],
            5555,
        ))))
        .body(Body::from(body))
        .unwrap()
}

pub fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}
