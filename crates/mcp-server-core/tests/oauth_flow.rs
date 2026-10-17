mod oauth_support;
use axum::Extension;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use mcp_server_core::jwt::{Claims, Payload, TokenKind};
use mcp_server_core::server::build_app;
use oauth_support::*;
use serde_json::json;
use std::time::Duration;

fn full_app(st: std::sync::Arc<mcp_server_core::oauth::AuthState>) -> axum::Router {
    build_app(
        st,
        axum::Router::new().route("/mcp", post(|| async { "mcp-ok" })),
    )
}

fn mcp_call(token: Option<&str>) -> Request<Body> {
    let mut b = Request::post("/mcp");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn unauthenticated_mcp_gets_401_with_resource_metadata() {
    let res = send(&full_app(state()), mcp_call(None)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let h = res.headers()["www-authenticate"].to_str().unwrap();
    assert!(h.starts_with("Bearer "));
    assert!(h.contains(
        "resource_metadata=\"https://mail.example.com/.well-known/oauth-protected-resource\""
    ));
}

#[tokio::test]
async fn healthz_needs_no_auth() {
    let res = send(&full_app(state()), get("/healthz")).await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn non_access_tokens_are_rejected() {
    let st = state();
    let app = full_app(st.clone());
    for kind in [TokenKind::Refresh, TokenKind::Code, TokenKind::Csrf] {
        let t = st
            .jwt
            .issue(kind, Some(Duration::from_secs(60)), Payload::default())
            .unwrap();
        assert_eq!(
            send(&app, mcp_call(Some(&t))).await.status(),
            StatusCode::UNAUTHORIZED,
            "{kind:?}"
        );
    }
    let client = st
        .jwt
        .issue(TokenKind::Client, None, Payload::default())
        .unwrap();
    assert_eq!(
        send(&app, mcp_call(Some(&client))).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn static_token_is_accepted_when_configured() {
    let app = full_app(state_with_static(Some("static-secret-token")));
    assert_eq!(
        send(&app, mcp_call(Some("static-secret-token")))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&app, mcp_call(Some("static-secret-tokeN")))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn handler_sees_claims_only_for_oauth_access_tokens() {
    let st = state_with_static(Some("static-secret-token"));
    let app = build_app(
        st.clone(),
        axum::Router::new().route(
            "/mcp",
            post(|claims: Option<Extension<Claims>>| async move {
                match claims {
                    Some(Extension(c)) => c.payload.client_id.unwrap_or_default(),
                    None => "no-claims".to_string(),
                }
            }),
        ),
    );
    let access = st
        .jwt
        .issue(
            TokenKind::Access,
            Some(Duration::from_secs(60)),
            Payload {
                client_id: Some("client-1".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let res = send(&app, mcp_call(Some(&access))).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_text(res).await, "client-1");
    let res = send(&app, mcp_call(Some("static-secret-token"))).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_text(res).await, "no-claims");
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let st = state_with_static(Some("t"));
    let req = Request::post("/register")
        .header("content-type", "application/json")
        .body(Body::from(vec![b' '; 2 * 1024 * 1024]))
        .unwrap();
    assert_eq!(
        send(&full_app(st), req).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn full_oauth_dance_reaches_mcp() {
    let st = state();
    let app = full_app(st.clone());

    let reg = body_json(
        send(
            &app,
            json_post(
                "/register",
                json!({"redirect_uris": ["https://app.example.com/cb"]}),
            ),
        )
        .await,
    )
    .await;
    let cid = reg["client_id"].as_str().unwrap().to_string();

    let verifier = "dBjftJeZ4CVP-mJ92K9fwhzDHvM5HDkyX5Dw1mEtK8k";
    let challenge = mcp_server_core::pkce::s256(verifier);
    let q = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("response_type", "code"),
            ("client_id", cid.as_str()),
            ("redirect_uri", "https://app.example.com/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "s1"),
        ])
        .finish();
    let html = body_text(send(&app, get(&format!("/authorize?{q}"))).await).await;
    let csrf_start = html.find("name=\"csrf\" value=\"").unwrap() + "name=\"csrf\" value=\"".len();
    let csrf = &html[csrf_start..csrf_start + html[csrf_start..].find('"').unwrap()];

    let res = send(
        &app,
        form_post(
            "/authorize",
            &[
                ("response_type", "code"),
                ("client_id", &cid),
                ("redirect_uri", "https://app.example.com/cb"),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("state", "s1"),
                ("scope", "mail"),
                ("resource", "https://mail.example.com/mcp"),
                ("csrf", csrf),
                ("password", PASSWORD),
            ],
        ),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FOUND);
    let loc = url::Url::parse(res.headers()["location"].to_str().unwrap()).unwrap();
    let code = loc
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();

    let tok = body_json(
        send(
            &app,
            form_post(
                "/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("client_id", &cid),
                    ("code_verifier", verifier),
                    ("redirect_uri", "https://app.example.com/cb"),
                ],
            ),
        )
        .await,
    )
    .await;
    let access = tok["access_token"].as_str().unwrap();

    let res = send(&app, mcp_call(Some(access))).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_text(res).await, "mcp-ok");
}
