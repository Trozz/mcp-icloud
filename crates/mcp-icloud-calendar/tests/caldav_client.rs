//! CalDAV client behaviour against a scripted local HTTP server: headers, auth scoping,
//! redirects, retries and status mapping.
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use mcp_icloud_calendar::caldav::caldav_settings;
use mcp_icloud_dav::client::{DavClient, Precondition};
use secrecy::SecretString;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay: Duration,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_string(),
        delay: Duration::ZERO,
    }
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: HeaderMap,
    body: String,
}

#[derive(Default)]
struct Script {
    replies: VecDeque<Reply>,
    seen: Vec<Seen>,
}

#[derive(Clone)]
struct Server {
    url: Url,
    script: Arc<Mutex<Script>>,
}

impl Server {
    async fn start() -> Server {
        let script = Arc::new(Mutex::new(Script::default()));
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(script.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Server {
            url: Url::parse(&format!("http://{addr}/")).unwrap(),
            script,
        }
    }

    fn push(&self, r: Reply) {
        self.script.lock().unwrap().replies.push_back(r);
    }

    fn seen(&self) -> Vec<Seen> {
        self.script.lock().unwrap().seen.clone()
    }

    fn at(&self, path: &str) -> Url {
        self.url.join(path).unwrap()
    }
}

async fn handle(
    State(script): State<Arc<Mutex<Script>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let next = {
        let mut s = script.lock().unwrap();
        s.seen.push(Seen {
            method: method.to_string(),
            path: uri.path().to_string(),
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
        s.replies.pop_front()
    };
    let Some(r) = next else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "script exhausted").into_response();
    };
    tokio::time::sleep(r.delay).await;
    let mut res = (StatusCode::from_u16(r.status).unwrap(), r.body).into_response();
    for (k, v) in r.headers {
        res.headers_mut().insert(k, v.parse().unwrap());
    }
    res
}

fn client(base: &Url) -> DavClient {
    DavClient::new(caldav_settings(
        base.clone(),
        "user@example.com".into(),
        SecretString::from("app-pass"),
    ))
    .unwrap()
}

const BASIC: &str = "Basic dXNlckBleGFtcGxlLmNvbTphcHAtcGFzcw==";
const MULTI: &str = r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/</href><propstat><prop><current-user-principal><href>/1234567890/principal/</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;

#[tokio::test]
async fn propfind_sends_depth_auth_and_xml_and_parses_the_answer() {
    let s = Server::start().await;
    s.push(reply(207, MULTI));
    let (answered, r) = client(&s.url)
        .propfind(&s.url, 0, "<x/>".into())
        .await
        .unwrap();
    assert_eq!(answered, s.url);
    assert_eq!(
        r[0].props.current_user_principal.as_deref(),
        Some("/1234567890/principal/")
    );
    let seen = s.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "PROPFIND");
    assert_eq!(seen[0].headers["depth"], "0");
    assert_eq!(seen[0].headers["authorization"], BASIC);
    assert_eq!(
        seen[0].headers["content-type"],
        "application/xml; charset=utf-8"
    );
    assert_eq!(seen[0].body, "<x/>");
}

#[tokio::test]
async fn report_uses_depth_one() {
    let s = Server::start().await;
    s.push(reply(207, MULTI));
    client(&s.url)
        .report(&s.at("/cal/"), "<q/>".into())
        .await
        .unwrap();
    let seen = s.seen();
    assert_eq!(seen[0].method, "REPORT");
    assert_eq!(seen[0].path, "/cal/");
    assert_eq!(seen[0].headers["depth"], "1");
}

#[tokio::test]
async fn create_sends_if_none_match_and_returns_the_etag() {
    let s = Server::start().await;
    let mut r = reply(201, "");
    r.headers.push(("etag", "\"e1\"".into()));
    s.push(r);
    let etag = client(&s.url)
        .put(
            &s.at("/cal/a.ics"),
            "BEGIN:VCALENDAR".into(),
            Precondition::IfNoneMatch,
        )
        .await
        .unwrap();
    assert_eq!(etag.as_deref(), Some("\"e1\""));
    let seen = s.seen();
    assert_eq!(seen[0].method, "PUT");
    assert_eq!(seen[0].headers["if-none-match"], "*");
    assert!(seen[0].headers.get("if-match").is_none());
    assert_eq!(
        seen[0].headers["content-type"],
        "text/calendar; charset=utf-8"
    );
}

#[tokio::test]
async fn update_sends_if_match_and_a_412_is_a_conflict_without_retry() {
    let s = Server::start().await;
    s.push(reply(412, ""));
    let e = client(&s.url)
        .put(
            &s.at("/cal/a.ics"),
            "x".into(),
            Precondition::IfMatch("\"e1\"".into()),
        )
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "conflict");
    let seen = s.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].headers["if-match"], "\"e1\"");
}

#[tokio::test]
async fn writes_are_never_retried_on_server_errors() {
    let s = Server::start().await;
    s.push(reply(503, ""));
    let e = client(&s.url)
        .delete(&s.at("/cal/a.ics"), "\"e1\"")
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert_eq!(s.seen().len(), 1);
    assert_eq!(s.seen()[0].headers["if-match"], "\"e1\"");
}

#[tokio::test]
async fn reads_retry_once_on_server_errors() {
    let s = Server::start().await;
    s.push(reply(503, ""));
    s.push(reply(207, MULTI));
    client(&s.url)
        .propfind(&s.url, 0, "<x/>".into())
        .await
        .unwrap();
    assert_eq!(s.seen().len(), 2);

    let s = Server::start().await;
    s.push(reply(500, ""));
    s.push(reply(502, ""));
    s.push(reply(207, MULTI));
    let e = client(&s.url).get(&s.at("/cal/a.ics")).await.unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert_eq!(s.seen().len(), 2, "exactly one retry");
}

#[tokio::test]
async fn reads_retry_once_on_timeouts() {
    let s = Server::start().await;
    let mut slow = reply(200, "late");
    slow.delay = Duration::from_millis(800);
    s.push(slow);
    let mut fast = reply(200, "BEGIN:VCALENDAR");
    fast.headers.push(("etag", "\"e2\"".into()));
    s.push(fast);
    let mut settings = caldav_settings(
        s.url.clone(),
        "user@example.com".into(),
        SecretString::from("app-pass"),
    );
    settings.request_timeout = Duration::from_millis(300);
    let c = DavClient::new(settings).unwrap();
    let got = c.get(&s.at("/cal/a.ics")).await.unwrap();
    assert_eq!(got.body, "BEGIN:VCALENDAR");
    assert_eq!(got.etag.as_deref(), Some("\"e2\""));
    assert_eq!(s.seen().len(), 2);
}

#[tokio::test]
async fn status_codes_map_to_error_kinds() {
    for (code, kind) in [
        (401, "auth"),
        (403, "read_only"),
        (404, "not_found"),
        (507, "quota"),
    ] {
        let s = Server::start().await;
        s.push(reply(code, ""));
        let e = client(&s.url).get(&s.at("/cal/a.ics")).await.unwrap_err();
        assert_eq!(e.kind(), kind, "HTTP {code}");
    }
}

#[tokio::test]
async fn same_origin_redirects_are_followed_with_auth() {
    let s = Server::start().await;
    let mut moved = reply(301, "");
    moved.headers.push(("location", "/elsewhere/".into()));
    s.push(moved);
    s.push(reply(207, MULTI));
    let (answered, _) = client(&s.url)
        .propfind(&s.url, 0, "<x/>".into())
        .await
        .unwrap();
    assert_eq!(answered, s.at("/elsewhere/"));
    let seen = s.seen();
    assert_eq!(seen[1].path, "/elsewhere/");
    assert_eq!(seen[1].method, "PROPFIND");
    assert_eq!(seen[1].headers["authorization"], BASIC);
}

#[tokio::test]
async fn redirects_to_other_origins_are_refused_before_any_request() {
    let s = Server::start().await;
    let other = Server::start().await;
    other.push(reply(207, MULTI));
    let mut moved = reply(302, "");
    moved
        .headers
        .push(("location", other.at("/steal/").to_string()));
    s.push(moved);
    let e = client(&s.url)
        .propfind(&s.url, 0, "<x/>".into())
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert!(e.to_string().contains("not a trusted CalDAV origin"), "{e}");
    assert!(
        other.seen().is_empty(),
        "credentials must not reach another origin"
    );
}

#[tokio::test]
async fn untrusted_urls_are_refused_outright() {
    let s = Server::start().await;
    let other = Server::start().await;
    let e = client(&s.url)
        .get(&other.at("/cal/a.ics"))
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert!(other.seen().is_empty());
}

#[tokio::test]
async fn redirect_loops_stop_after_five_hops() {
    let s = Server::start().await;
    for _ in 0..7 {
        let mut moved = reply(307, "");
        moved.headers.push(("location", "/again/".into()));
        s.push(moved);
    }
    let e = client(&s.url).get(&s.at("/start/")).await.unwrap_err();
    assert!(e.to_string().contains("too many redirects"), "{e}");
    assert_eq!(s.seen().len(), 6);
}
