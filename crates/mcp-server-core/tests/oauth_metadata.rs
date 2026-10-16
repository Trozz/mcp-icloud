mod oauth_support;
use axum::http::StatusCode;
use oauth_support::*;
use serde_json::json;

#[tokio::test]
async fn protected_resource_metadata() {
    let app = app(state());
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let res = send(&app, get(path)).await;
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["resource"], "https://mail.example.com/mcp");
        assert_eq!(
            v["authorization_servers"],
            json!(["https://mail.example.com"])
        );
    }
}

#[tokio::test]
async fn authorization_server_metadata() {
    let v = body_json(
        send(
            &app(state()),
            get("/.well-known/oauth-authorization-server"),
        )
        .await,
    )
    .await;
    assert_eq!(v["issuer"], "https://mail.example.com");
    assert_eq!(
        v["authorization_endpoint"],
        "https://mail.example.com/authorize"
    );
    assert_eq!(v["token_endpoint"], "https://mail.example.com/token");
    assert_eq!(
        v["registration_endpoint"],
        "https://mail.example.com/register"
    );
    assert_eq!(v["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(v["token_endpoint_auth_methods_supported"], json!(["none"]));
    assert_eq!(
        v["grant_types_supported"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(v["response_types_supported"], json!(["code"]));
}

#[tokio::test]
async fn register_returns_signed_client_id() {
    let st = state();
    let res = send(&app(st.clone()), json_post("/register", json!({"redirect_uris": ["https://claude.ai/api/mcp/auth_callback"], "client_name": "Claude"}))).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let v = body_json(res).await;
    let client_id = v["client_id"].as_str().unwrap();
    let claims = st
        .jwt
        .verify(client_id, mcp_server_core::jwt::TokenKind::Client)
        .unwrap();
    assert_eq!(
        claims.payload.redirect_uris.unwrap(),
        vec!["https://claude.ai/api/mcp/auth_callback"]
    );
    assert_eq!(v["token_endpoint_auth_method"], "none");
    assert_eq!(
        v["redirect_uris"],
        json!(["https://claude.ai/api/mcp/auth_callback"])
    );
}

#[tokio::test]
async fn register_rejects_bad_redirect_uris() {
    let app = app(state());
    for uris in [
        json!([]),
        json!(["http://evil.example.com/cb"]),
        json!(["not a url"]),
        json!(["https://a.example.com/cb#frag"]),
        json!(["javascript:alert(1)"]),
    ] {
        let res = send(&app, json_post("/register", json!({"redirect_uris": uris}))).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{uris}");
        assert_eq!(body_json(res).await["error"], "invalid_redirect_uri");
    }
}

#[tokio::test]
async fn register_allows_loopback_http() {
    let app = app(state());
    for uri in [
        "http://localhost:33418/callback",
        "http://127.0.0.1:9000/cb",
        "http://[::1]:9000/cb",
    ] {
        let res = send(
            &app,
            json_post("/register", json!({"redirect_uris": [uri]})),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED, "{uri}");
    }
}
