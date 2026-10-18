mod support;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use mcp_icloud_mail::MailService;
use mcp_icloud_mail::smtp::{RecordingSender, Sender};
use mcp_server_core::config::{CoreConfig, EnvReader};
use mcp_server_core::oauth::AuthState;
use mcp_server_core::server::build_app;
use mcp_server_core::service::{HttpSettings, Service, ServiceContext};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use support::fake_imap::{FakeImap, FakeState};
use support::tools::{imap, settings};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const TOKEN: &str = "static-test-token-0123456789";
const HOST: &str = "mail.example.com";

/// Builds the full application (auth, OAuth routes, mail at /mail) the way the server does, against the fake.
async fn app(allow_send: bool) -> (axum::Router, FakeImap) {
    let mut st = FakeState::icloud_like();
    st.add_message(
        "INBOX",
        b"From: a@example.com\r\nSubject: hello\r\nMessage-ID: <h1@example.com>\r\n\r\nhi\r\n",
        &[],
    );
    let fake = FakeImap::start(st).await;
    let vars: HashMap<&str, &str> = HashMap::from([
        ("PUBLIC_URL", "https://mail.example.com"),
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
    let sender: Option<Arc<dyn Sender>> =
        allow_send.then(|| Arc::new(RecordingSender::default()) as Arc<dyn Sender>);
    let mail = Arc::new(MailService::new(imap(&fake), sender, settings(1024 * 1024)));
    let auth = AuthState::new(core.oauth, &["mail"]);
    (build_app(auth, vec![("mail", mail.router(&ctx))]), fake)
}

fn post(session: Option<&str>) -> axum::http::request::Builder {
    let mut b = Request::post("/mail")
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
    assert_eq!(init["result"]["serverInfo"]["name"], "mcp-icloud-mail");
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
    assert_eq!(names.len(), 14, "{names:?}");
    assert!(!names.contains(&"send_draft".to_string()));
    let delete = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "delete_messages")
        .unwrap();
    assert_eq!(delete["annotations"]["destructiveHint"], true);
    for (name, destructive) in [("delete_mailbox", true), ("rename_mailbox", false)] {
        let tool = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], false, "{name}");
        assert_eq!(
            tool["annotations"]["destructiveHint"], destructive,
            "{name}"
        );
    }

    let res = call(
        &app,
        &session,
        3,
        "tools/call",
        json!({"name": "search_messages", "arguments": {}}),
    )
    .await;
    assert_eq!(res["result"]["isError"], false);
    let page: Value =
        serde_json::from_str(res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(page["messages"][0]["subject"], "hello");
}

#[tokio::test]
async fn send_draft_listed_only_when_sending_is_allowed() {
    let (app, _fake) = app(true).await;
    let session = handshake(&app).await;
    let names = tool_names(&call(&app, &session, 2, "tools/list", json!({})).await);
    assert!(names.contains(&"send_draft".to_string()), "{names:?}");
}

#[tokio::test]
async fn foreign_origin_is_rejected() {
    let (app, _fake) = app(false).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}
    }});
    let res = app
        .clone()
        .oneshot(
            post(None)
                .header("origin", "https://evil.example.net")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let res = app
        .clone()
        .oneshot(
            post(None)
                .header("origin", "https://mail.example.com")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "own origin must be accepted");
}

#[tokio::test]
async fn wrong_host_is_rejected() {
    let (app, _fake) = app(false).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}
    }});
    let mut req = request(body, None);
    req.headers_mut()
        .insert("host", "attacker.example.net".parse().unwrap());
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn missing_token_is_unauthorized() {
    let (app, _fake) = app(false).await;
    let mut req = request(
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        None,
    );
    req.headers_mut().remove("authorization");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
