//! `discover` and `Directory` against a local HTTP server that serves the recorded
//! principal, home and calendar listings.
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use mcp_icloud_calendar::caldav::client::{CaldavClient, CaldavSettings};
use mcp_icloud_calendar::caldav::discovery::{Directory, discover};
use secrecy::SecretString;
use std::sync::{Arc, Mutex};
use url::Url;

const PRINCIPAL: &str = include_str!("fixtures/caldav-principal.xml");
const HOME: &str = include_str!("fixtures/caldav-home.xml");
const CALENDARS: &str = include_str!("fixtures/caldav-calendars.xml");
const ICLOUD_ORIGIN: &str = "https://p99-caldav.icloud.com:443";

#[derive(Default)]
struct Dav {
    home: String,
    seen: Vec<(String, String)>,
}

struct Server {
    url: Url,
    dav: Arc<Mutex<Dav>>,
}

impl Server {
    async fn start(home_origin: Option<&str>) -> Server {
        let dav = Arc::new(Mutex::new(Dav::default()));
        let app = axum::Router::new().fallback(handle).with_state(dav.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let origin = format!("http://{addr}");
        dav.lock().unwrap().home = HOME.replace(ICLOUD_ORIGIN, home_origin.unwrap_or(&origin));
        Server {
            url: Url::parse(&format!("{origin}/")).unwrap(),
            dav,
        }
    }

    fn seen(&self) -> Vec<(String, String)> {
        self.dav.lock().unwrap().seen.clone()
    }
}

async fn handle(State(dav): State<Arc<Mutex<Dav>>>, uri: Uri, headers: HeaderMap) -> Response {
    let mut dav = dav.lock().unwrap();
    let depth = headers
        .get("depth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    dav.seen.push((uri.path().to_string(), depth));
    let body = match uri.path() {
        "/" => PRINCIPAL.to_string(),
        "/1234567890/principal/" => dav.home.clone(),
        "/1234567890/calendars/" => CALENDARS.to_string(),
        _ => return (StatusCode::NOT_FOUND, "").into_response(),
    };
    (StatusCode::MULTI_STATUS, body).into_response()
}

fn client(base: &Url) -> CaldavClient {
    CaldavClient::new(CaldavSettings::new(
        base.clone(),
        "user@example.com".into(),
        SecretString::from("app-pass"),
    ))
    .unwrap()
}

#[tokio::test]
async fn discovery_walks_principal_home_and_event_calendars() {
    let s = Server::start(None).await;
    let a = discover(&client(&s.url), "login@example.net")
        .await
        .unwrap();

    assert_eq!(a.home, s.url.join("/1234567890/calendars/").unwrap());
    let ids: Vec<&str> = a.calendars.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "/1234567890/calendars/holidays/",
            "/1234567890/calendars/home/"
        ]
    );
    let home = a.calendar("Home & Family").unwrap();
    assert_eq!(home.url, s.url.join("/1234567890/calendars/home/").unwrap());
    assert_eq!(home.color.as_deref(), Some("#999999FF"));
    assert!(home.writable);
    assert!(!a.calendar("Holidays").unwrap().writable);

    assert!(a.me.matches("mailto:user.alias@example.org"));
    assert!(a.me.matches("USER@example.com"));
    assert!(a.me.matches("mailto:login@example.net"));
    assert!(!a.me.matches("mailto:someone@example.com"));

    assert_eq!(
        s.seen(),
        [
            ("/".to_string(), "0".to_string()),
            ("/1234567890/principal/".to_string(), "0".to_string()),
            ("/1234567890/calendars/".to_string(), "1".to_string()),
        ]
    );
}

#[tokio::test]
async fn an_untrusted_calendar_home_is_never_contacted() {
    let s = Server::start(Some(ICLOUD_ORIGIN)).await;
    let e = discover(&client(&s.url), "user@example.com")
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert!(e.to_string().contains("not a trusted CalDAV origin"), "{e}");
    assert_eq!(s.seen().len(), 2);
}

#[tokio::test]
async fn the_directory_caches_until_refreshed() {
    let s = Server::start(None).await;
    let d = Directory::new(Arc::new(client(&s.url)), "user@example.com".into());
    assert_eq!(d.client().base(), &s.url);

    let first = d.account().await.unwrap();
    let again = d.account().await.unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(s.seen().len(), 3);

    let refreshed = d.refresh().await.unwrap();
    assert!(!Arc::ptr_eq(&first, &refreshed));
    assert_eq!(refreshed.calendars, first.calendars);
    assert_eq!(s.seen().len(), 6);

    d.invalidate().await;
    assert_eq!(s.seen().len(), 6);
    let after = d.account().await.unwrap();
    assert!(!Arc::ptr_eq(&refreshed, &after));
    assert_eq!(s.seen().len(), 9);
}
