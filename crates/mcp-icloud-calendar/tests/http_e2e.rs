mod support;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use mcp_icloud_calendar::CalendarService;
use mcp_server_core::config::{CoreConfig, EnvReader};
use mcp_server_core::oauth::AuthState;
use mcp_server_core::server::build_app;
use mcp_server_core::service::{HttpSettings, Service, ServiceContext};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use support::fake_caldav::{FakeCalDav, FakeState};
use support::tools::{directory, settings};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const TOKEN: &str = "static-test-token-0123456789";
const HOST: &str = "cal.example.com";

/// Builds the full application (auth, OAuth routes, calendar at /calendar) the way the server
/// does, against the fake.
async fn app(allow_invites: bool) -> (axum::Router, FakeCalDav) {
    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let vars: HashMap<&str, &str> = HashMap::from([
        ("PUBLIC_URL", "https://cal.example.com"),
        ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
        ("OAUTH_ADMIN_PASSWORD", "a long password"),
        ("MCP_AUTH_TOKEN", TOKEN),
        ("INSECURE_HTTP", "true"),
    ]);
    let mut env = EnvReader::new(move |k: &str| vars.get(k).map(|v| v.to_string()));
    let core = CoreConfig::from_env(&mut env, "mcp-icloud", "mcp_icloud")
        .unwrap_or_else(|| panic!("config errors: {:?}", env.errors));
    let ctx = ServiceContext {
        http: HttpSettings::from_core(&core),
        shutdown: CancellationToken::new(),
    };
    let calendar = Arc::new(CalendarService::new(
        directory(&fake),
        settings(allow_invites),
    ));
    let auth = AuthState::new(core.oauth, &["calendar"]);
    (
        build_app(auth, vec![("calendar", calendar.router(&ctx))]),
        fake,
    )
}

fn post(session: Option<&str>) -> axum::http::request::Builder {
    let mut b = Request::post("/calendar")
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(s) = session {
        b = b
            .header("mcp-session-id", s)
            .header("mcp-protocol-version", "2025-06-18");
    }
    b
}

fn request(body: Value, session: Option<&str>) -> Request<Body> {
    post(session).body(Body::from(body.to_string())).unwrap()
}

/// Reads an SSE (or plain JSON) response until the first complete JSON-RPC message.
async fn rpc_message(res: Response<Body>) -> Value {
    let mut body = res.into_body();
    let mut buf = String::new();
    loop {
        if let Ok(v) = serde_json::from_str::<Value>(buf.trim()) {
            return v;
        }
        for event in buf.split("\n\n") {
            for line in event.lines() {
                if let Some(data) = line.strip_prefix("data:")
                    && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                    && v.get("jsonrpc").is_some()
                {
                    return v;
                }
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

/// initialize + notifications/initialized; returns the session id.
async fn handshake(app: &axum::Router) -> String {
    let res = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "e2e", "version": "1"}
            }}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let session = res
        .headers()
        .get("mcp-session-id")
        .expect("session id header")
        .to_str()
        .unwrap()
        .to_string();
    let init = rpc_message(res).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "mcp-icloud-calendar");
    let res = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(&session),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    session
}

async fn call(app: &axum::Router, session: &str, id: u64, method: &str, params: Value) -> Value {
    let res = app
        .clone()
        .oneshot(request(
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            Some(session),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    rpc_message(res).await
}

fn tool_names(list: &Value) -> Vec<String> {
    list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn tools_list_and_call_over_http() {
    let (app, _fake) = app(false).await;
    let session = handshake(&app).await;
    let list = call(&app, &session, 2, "tools/list", json!({})).await;
    let names = tool_names(&list);
    assert_eq!(names.len(), 11, "{names:?}");
    assert!(!names.contains(&"respond_to_invitation".to_string()));
    let tools = list["result"]["tools"].as_array().unwrap();
    let annotation = |name: &str, key: &str| {
        tools.iter().find(|t| t["name"] == name).unwrap()["annotations"][key].clone()
    };
    assert_eq!(annotation("delete_event", "destructiveHint"), true);
    assert_eq!(annotation("update_event", "destructiveHint"), true);
    assert_eq!(annotation("list_events", "readOnlyHint"), true);
    assert_eq!(annotation("set_timezone", "idempotentHint"), true);
    assert_eq!(annotation("create_calendar", "readOnlyHint"), false);
    assert_eq!(annotation("create_calendar", "destructiveHint"), false);

    let res = call(
        &app,
        &session,
        3,
        "tools/call",
        json!({"name": "list_calendars", "arguments": {}}),
    )
    .await;
    assert_eq!(res["result"]["isError"], false);
    let body: Value =
        serde_json::from_str(res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["calendars"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn wrong_host_and_missing_token_are_rejected() {
    let (app, _fake) = app(false).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}
    }});
    let mut req = request(body.clone(), None);
    req.headers_mut()
        .insert("host", "attacker.example.net".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut req = request(body, None);
    req.headers_mut().remove("authorization");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        res.headers()["www-authenticate"].to_str().unwrap(),
        "Bearer resource_metadata=\"https://cal.example.com/.well-known/oauth-protected-resource/calendar\""
    );
}
