use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use mcp_icloud::{SERVICES, app, load};
use mcp_server_core::service::{HttpSettings, ServiceContext, ServiceEnv};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const TOKEN: &str = "static-test-token-0123456789";
const HOST: &str = "icloud.example.com";

/// Builds the application exactly as `main` does, from environment variables only.
fn server(extra: &[(&str, &str)]) -> axum::Router {
    let mut vars: HashMap<String, String> = [
        ("PUBLIC_URL", "https://icloud.example.com"),
        ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
        ("OAUTH_ADMIN_PASSWORD", "a long password"),
        ("MCP_AUTH_TOKEN", TOKEN),
        ("INSECURE_HTTP", "true"),
        ("ICLOUD_EMAIL", "me@icloud.com"),
        ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    let mut env = ServiceEnv::new(Box::new(move |k: &str| vars.get(k).cloned()));
    let loaded = load(&mut env, SERVICES).unwrap_or_else(|| panic!("{:?}", env.errors));
    let ctx = ServiceContext {
        http: HttpSettings::from_core(&loaded.core),
        shutdown: CancellationToken::new(),
    };
    app(loaded.core.oauth, &loaded.services, &ctx)
}

fn rpc(path: &str, body: Value, token: Option<&str>, session: Option<&str>) -> Request<Body> {
    let mut b = Request::post(path)
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    if let Some(s) = session {
        b = b
            .header("mcp-session-id", s)
            .header("mcp-protocol-version", "2025-06-18");
    }
    b.body(Body::from(body.to_string())).unwrap()
}

/// Reads an SSE (or plain JSON) response until the first complete JSON-RPC message.
async fn rpc_message(res: Response<Body>) -> Value {
    let mut body = res.into_body();
    let mut buf = String::new();
    loop {
        if let Ok(v) = serde_json::from_str::<Value>(buf.trim()) {
            return v;
        }
        for line in buf.lines() {
            if let Some(data) = line.strip_prefix("data:")
                && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                && v.get("jsonrpc").is_some()
            {
                return v;
            }
        }
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("response within 5 s")
            .expect("body not finished before a message")
            .expect("body frame");
        if let Ok(data) = frame.into_data() {
            buf.push_str(&String::from_utf8_lossy(&data));
        }
    }
}

fn initialize() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "e2e", "version": "1"}
    }})
}

#[tokio::test]
async fn healthz_needs_no_auth() {
    let res = server(&[])
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn mail_is_served_at_its_path_with_the_static_token() {
    let app = server(&[]);
    let res = app
        .clone()
        .oneshot(rpc("/mail", initialize(), Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let session = res.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    let init = rpc_message(res).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "mcp-icloud-mail");
    let res = app
        .clone()
        .oneshot(rpc(
            "/mail",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(TOKEN),
            Some(&session),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let res = app
        .oneshot(rpc(
            "/mail",
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
            Some(TOKEN),
            Some(&session),
        ))
        .await
        .unwrap();
    let list = rpc_message(res).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 12, "{tools:?}");
}

#[tokio::test]
async fn mail_without_a_token_points_to_its_resource_metadata() {
    let app = server(&[]);
    let res = app
        .clone()
        .oneshot(rpc("/mail", initialize(), None, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        res.headers()["www-authenticate"].to_str().unwrap(),
        "Bearer resource_metadata=\"https://icloud.example.com/.well-known/oauth-protected-resource/mail\""
    );
    let res = app
        .oneshot(
            Request::get("/.well-known/oauth-protected-resource/mail")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["resource"], "https://icloud.example.com/mail");
    assert_eq!(v["scopes_supported"], json!(["mail"]));
}

#[tokio::test]
async fn mail_sub_paths_are_guarded_too() {
    let app = server(&[]);
    let metadata = "Bearer resource_metadata=\"https://icloud.example.com/.well-known/oauth-protected-resource/mail\"";
    for (path, token) in [
        ("/mail/unknown", None),
        ("/mail/x/y", None),
        ("/mail/unknown", Some("not-a-valid-token")),
        ("/mail", Some("not-a-valid-token")),
    ] {
        let res = app
            .clone()
            .oneshot(rpc(path, initialize(), token, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path} {token:?}");
        assert!(
            res.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .starts_with(metadata),
            "{path} {token:?}"
        );
    }
}

#[tokio::test]
async fn old_mcp_path_is_not_served() {
    let res = server(&[])
        .oneshot(rpc("/mcp", initialize(), Some(TOKEN), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_service_selection_fails_to_load() {
    let mut env = ServiceEnv::new(Box::new(|k: &str| match k {
        "ICLOUD_SERVICES" => Some("nope".to_string()),
        _ => None,
    }));
    assert!(load(&mut env, SERVICES).is_none());
    assert!(
        env.errors
            .iter()
            .any(|e| e.starts_with("ICLOUD_SERVICES contains an unknown service: nope")),
        "{:?}",
        env.errors
    );
}
