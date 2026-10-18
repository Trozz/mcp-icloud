//! CardDAV behaviour of the shared client and home discovery against a scripted local server.
//! The CalDAV behaviour (headers, redirects, retries, status mapping) is covered by the calendar
//! crate's `tests/caldav_client.rs`.
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use mcp_icloud_dav::DavError;
use mcp_icloud_dav::client::{DavClient, DavSettings, Precondition, Protocol};
use mcp_icloud_dav::discovery::{HomeSet, discover_home};
use mcp_icloud_dav::xml::sync_collection;
use secrecy::SecretString;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use url::Url;

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_string(),
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
    let mut res = (StatusCode::from_u16(r.status).unwrap(), r.body).into_response();
    for (k, v) in r.headers {
        res.headers_mut().insert(k, v.parse().unwrap());
    }
    res
}

fn carddav(base: &Url) -> DavClient {
    DavClient::new(DavSettings::new(
        Protocol::CardDav,
        base.clone(),
        "user@example.com".into(),
        SecretString::from("app-pass"),
        "mcp-icloud-contacts/0.0.0-test".into(),
    ))
    .unwrap()
}

const PRINCIPAL: &str = r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/</href><propstat><prop><current-user-principal><href>/1234567890/principal/</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;

fn address_book_home(url: &Url) -> String {
    format!(
        r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/1234567890/principal/</href><propstat><prop><addressbook-home-set xmlns="urn:ietf:params:xml:ns:carddav"><href xmlns="DAV:">{url}1234567890/carddavhome/</href></addressbook-home-set></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#
    )
}

const SYNC: &str = r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/1234567890/carddavhome/card/a.vcf</href><propstat><prop><getetag>"e1"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/1234567890/carddavhome/card/gone.vcf</href><status>HTTP/1.1 404 Not Found</status></response><sync-token>token-2</sync-token></multistatus>"#;

#[tokio::test]
async fn carddav_puts_send_vcard_content_type_and_the_user_agent() {
    let s = Server::start().await;
    let mut created = reply(201, "Created");
    created.headers.push(("etag", "\"e1\"".into()));
    s.push(created);
    let etag = carddav(&s.url)
        .put(
            &s.at("/1234567890/carddavhome/card/a.vcf"),
            "BEGIN:VCARD\r\nEND:VCARD\r\n".into(),
            Precondition::IfNoneMatch,
        )
        .await
        .unwrap();
    assert_eq!(etag.as_deref(), Some("\"e1\""));
    let seen = s.seen();
    assert_eq!(seen[0].method, "PUT");
    assert_eq!(seen[0].headers["content-type"], "text/vcard; charset=utf-8");
    assert_eq!(seen[0].headers["if-none-match"], "*");
    assert_eq!(
        seen[0].headers["user-agent"],
        "mcp-icloud-contacts/0.0.0-test"
    );
}

#[tokio::test]
async fn sync_report_sends_depth_one_and_returns_changes_and_the_token() {
    let s = Server::start().await;
    s.push(reply(207, SYNC));
    let m = carddav(&s.url)
        .sync_report(
            &s.at("/1234567890/carddavhome/card/"),
            sync_collection(Some("token-1")),
        )
        .await
        .unwrap();
    assert_eq!(m.sync_token.as_deref(), Some("token-2"));
    assert_eq!(m.responses.len(), 2);
    assert_eq!(m.responses[1].status, Some(404));
    let seen = s.seen();
    assert_eq!(seen[0].method, "REPORT");
    assert_eq!(seen[0].headers["depth"], "1");
    assert!(
        seen[0]
            .body
            .contains("<d:sync-token>token-1</d:sync-token>")
    );
}

#[tokio::test]
async fn an_invalid_sync_token_is_a_conflict() {
    let s = Server::start().await;
    s.push(reply(
        412,
        r#"<?xml version="1.0"?><error xmlns="DAV:"><valid-sync-token/></error>"#,
    ));
    let e = carddav(&s.url)
        .sync_report(
            &s.at("/1234567890/carddavhome/card/"),
            sync_collection(Some("stale")),
        )
        .await
        .unwrap_err();
    assert_eq!(e, DavError::Conflict);
    assert_eq!(s.seen().len(), 1, "a 412 is not retried");
}

#[tokio::test]
async fn a_carddav_client_names_carddav_when_refusing_an_origin() {
    let e = carddav(&Url::parse("http://127.0.0.1:9/").unwrap())
        .get(&Url::parse("https://example.com/card.vcf").unwrap())
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert!(
        e.to_string().contains("not a trusted CardDAV origin"),
        "{e}"
    );
}

#[tokio::test]
async fn discover_home_walks_principal_and_address_book_home() {
    let s = Server::start().await;
    s.push(reply(207, PRINCIPAL));
    s.push(reply(207, &address_book_home(&s.url)));
    let home = discover_home(&carddav(&s.url), HomeSet::AddressBook)
        .await
        .unwrap();
    assert_eq!(home.principal, s.at("/1234567890/principal/"));
    assert_eq!(home.home, s.at("/1234567890/carddavhome/"));
    assert!(home.principal_props.addressbook_home_set.is_some());
    let seen = s.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/");
    assert_eq!(seen[0].headers["depth"], "0");
    assert!(seen[0].body.contains("current-user-principal"));
    assert_eq!(seen[1].path, "/1234567890/principal/");
    assert_eq!(seen[1].headers["depth"], "0");
    assert!(seen[1].body.contains("<card:addressbook-home-set/>"));
}

#[tokio::test]
async fn a_principal_without_an_address_book_home_is_an_upstream_error() {
    let s = Server::start().await;
    s.push(reply(207, PRINCIPAL));
    s.push(reply(
        207,
        r#"<?xml version="1.0"?><multistatus xmlns="DAV:"><response><href>/1234567890/principal/</href><propstat><prop><addressbook-home-set xmlns="urn:ietf:params:xml:ns:carddav"/></prop><status>HTTP/1.1 404 Not Found</status></propstat></response></multistatus>"#,
    ));
    let e = discover_home(&carddav(&s.url), HomeSet::AddressBook)
        .await
        .unwrap_err();
    assert_eq!(
        e,
        DavError::Upstream("the server did not name an address book home".into())
    );
}
