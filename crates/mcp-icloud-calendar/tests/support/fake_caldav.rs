//! In-memory CalDAV server shaped like iCloud: a principal, a calendar home with event
//! calendars, a VTODO-only list, a scheduling inbox and outbox. Real ETags and If-Match /
//! If-None-Match semantics; calendars can be made (MKCALENDAR), patched (PROPPATCH), listed
//! (Depth 1 PROPFIND) and deleted. Every request is logged for assertions.
//!
//! `calendar-query` returns every resource of the calendar (no time-range filtering): the
//! server under test expands recurrences and filters by range itself.
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use url::Url;

pub const PRINCIPAL: &str = "/1234567890/principal/";
pub const HOME: &str = "/1234567890/calendars/";
pub const EMAIL: &str = "user@example.com";
pub const PASSWORD: &str = "app-pass";
/// `Basic base64("user@example.com:app-pass")`.
pub const AUTH: &str = "Basic dXNlckBleGFtcGxlLmNvbTphcHAtcGFzcw==";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Calendar,
    Inbox,
    Outbox,
}

#[derive(Debug, Clone)]
pub struct Resource {
    pub etag: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct FakeCalendar {
    pub name: String,
    pub color: Option<String>,
    pub components: Vec<&'static str>,
    pub writable: bool,
    pub kind: Kind,
    pub resources: BTreeMap<String, Resource>,
}

impl FakeCalendar {
    pub fn events(name: &str, writable: bool) -> Self {
        FakeCalendar {
            name: name.into(),
            color: Some("#1BADF8FF".into()),
            components: vec!["VEVENT"],
            writable,
            kind: Kind::Calendar,
            resources: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Logged {
    pub method: String,
    pub path: String,
    pub authorized: bool,
    pub depth: Option<String>,
    pub if_match: Option<String>,
    pub if_none_match: Option<String>,
    pub body: String,
}

#[derive(Debug)]
pub struct FakeState {
    pub calendars: BTreeMap<String, FakeCalendar>,
    pub log: Vec<Logged>,
    /// Next failures to inject: (method, status). The first entry whose method matches is used.
    pub fail: VecDeque<(&'static str, u16)>,
    /// Answer PROPFIND on `/` with a redirect to `/root/`, which then serves the principal.
    pub redirect_root: bool,
    /// Origin used in `calendar-home-set` (default: this server's own origin).
    pub home_origin: Option<String>,
    /// Delay for every REPORT, to observe concurrency.
    pub report_delay: Duration,
    pub reports_in_flight: usize,
    pub max_reports_in_flight: usize,
    /// Answer PROPPATCH with 207 and a 403 propstat, changing nothing (a refused property).
    pub ignore_proppatch: bool,
    etag_counter: u64,
}

impl FakeState {
    /// Home and Work (writable), Holidays (read-only), a VTODO-only Reminders list, inbox and
    /// outbox: the collection mix iCloud returns.
    pub fn icloud_like() -> Self {
        let mut calendars = BTreeMap::new();
        calendars.insert("home".into(), FakeCalendar::events("Home", true));
        calendars.insert("work".into(), FakeCalendar::events("Work", true));
        calendars.insert("holidays".into(), FakeCalendar::events("Holidays", false));
        calendars.insert(
            "reminders".into(),
            FakeCalendar {
                components: vec!["VTODO"],
                ..FakeCalendar::events("Reminders", true)
            },
        );
        calendars.insert(
            "inbox".into(),
            FakeCalendar {
                kind: Kind::Inbox,
                color: None,
                ..FakeCalendar::events("", true)
            },
        );
        calendars.insert(
            "outbox".into(),
            FakeCalendar {
                kind: Kind::Outbox,
                color: None,
                components: vec!["VEVENT", "VTODO"],
                ..FakeCalendar::events("", false)
            },
        );
        FakeState {
            calendars,
            log: Vec::new(),
            fail: VecDeque::new(),
            redirect_root: false,
            home_origin: None,
            report_delay: Duration::ZERO,
            reports_in_flight: 0,
            max_reports_in_flight: 0,
            ignore_proppatch: false,
            etag_counter: 0,
        }
    }

    fn next_etag(&mut self) -> String {
        self.etag_counter += 1;
        format!("\"etag-{}\"", self.etag_counter)
    }

    /// Stores (or replaces) a resource; returns its ETag.
    pub fn put(&mut self, calendar: &str, name: &str, ics: &str) -> String {
        let etag = self.next_etag();
        self.calendars
            .get_mut(calendar)
            .expect("known calendar")
            .resources
            .insert(
                name.into(),
                Resource {
                    etag: etag.clone(),
                    body: ics.replace("\r\n", "\n").replace('\n', "\r\n"),
                },
            );
        etag
    }

    pub fn resource(&self, calendar: &str, name: &str) -> Option<Resource> {
        self.calendars.get(calendar)?.resources.get(name).cloned()
    }

    pub fn resource_names(&self, calendar: &str) -> Vec<String> {
        self.calendars[calendar].resources.keys().cloned().collect()
    }

    pub fn count(&self, method: &str) -> usize {
        self.log.iter().filter(|l| l.method == method).count()
    }
}

#[derive(Clone)]
pub struct FakeCalDav {
    pub url: Url,
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeCalDav {
    pub async fn start(state: FakeState) -> FakeCalDav {
        let state = Arc::new(Mutex::new(state));
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FakeCalDav {
            url: Url::parse(&format!("http://{addr}/")).unwrap(),
            state,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    pub fn origin(&self) -> String {
        self.url.origin().ascii_serialization()
    }

    /// Event id (href path) of a resource in one of the fake's calendars.
    pub fn event_id(calendar: &str, name: &str) -> String {
        format!("{HOME}{calendar}/{name}")
    }
}

fn multistatus(responses: &[String]) -> Response {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<multistatus xmlns=\"DAV:\">\n{}\n</multistatus>",
        responses.join("\n")
    );
    (
        StatusCode::MULTI_STATUS,
        [("content-type", "application/xml; charset=UTF-8")],
        body,
    )
        .into_response()
}

fn ok_propstat(href: &str, props: &str) -> String {
    format!(
        "<response><href>{href}</href><propstat><prop>{props}</prop><status>HTTP/1.1 200 OK</status></propstat></response>"
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn collection_xml(slug: &str, c: &FakeCalendar) -> String {
    let kind = match c.kind {
        Kind::Calendar => "<calendar xmlns=\"urn:ietf:params:xml:ns:caldav\"/>",
        Kind::Inbox => "<schedule-inbox xmlns=\"urn:ietf:params:xml:ns:caldav\"/>",
        Kind::Outbox => "<schedule-outbox xmlns=\"urn:ietf:params:xml:ns:caldav\"/>",
    };
    let comps: String = c
        .components
        .iter()
        .map(|n| format!("<comp xmlns=\"urn:ietf:params:xml:ns:caldav\" name='{n}'/>"))
        .collect();
    let privs = if c.writable {
        "<privilege><read/></privilege><privilege><write/></privilege>"
    } else {
        "<privilege><read/></privilege>"
    };
    let mut props = format!(
        "<resourcetype><collection/>{kind}</resourcetype><supported-calendar-component-set xmlns=\"urn:ietf:params:xml:ns:caldav\">{comps}</supported-calendar-component-set><current-user-privilege-set>{privs}</current-user-privilege-set>"
    );
    if !c.name.is_empty() {
        props.push_str(&format!(
            "<displayname>{}</displayname>",
            xml_escape(&c.name)
        ));
    }
    if let Some(color) = &c.color {
        props.push_str(&format!(
            "<calendar-color xmlns=\"http://apple.com/ns/ical/\">{color}</calendar-color>"
        ));
    }
    ok_propstat(&format!("{HOME}{slug}/"), &props)
}

fn status(code: u16) -> Response {
    StatusCode::from_u16(code).unwrap().into_response()
}

async fn handle(
    State(state): State<Arc<Mutex<FakeState>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let path = uri.path().to_string();
    let authorized = header("authorization").as_deref() == Some(AUTH);
    let host = header("host").unwrap_or_default();
    let is_report = method.as_str() == "REPORT";
    let delay = {
        let mut s = state.lock().unwrap();
        s.log.push(Logged {
            method: method.to_string(),
            path: path.clone(),
            authorized,
            depth: header("depth"),
            if_match: header("if-match"),
            if_none_match: header("if-none-match"),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
        if !authorized {
            return status(401);
        }
        if let Some(i) = s.fail.iter().position(|(m, _)| *m == method.as_str()) {
            let (_, code) = s.fail.remove(i).unwrap();
            return status(code);
        }
        if is_report {
            s.reports_in_flight += 1;
            s.max_reports_in_flight = s.max_reports_in_flight.max(s.reports_in_flight);
        }
        s.report_delay
    };
    if is_report {
        tokio::time::sleep(delay).await;
        let res = respond(&state, &method, &path, &host, &headers, &body);
        state.lock().unwrap().reports_in_flight -= 1;
        return res;
    }
    respond(&state, &method, &path, &host, &headers, &body)
}

fn respond(
    state: &Arc<Mutex<FakeState>>,
    method: &Method,
    path: &str,
    host: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let mut s = state.lock().unwrap();
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    match method.as_str() {
        "PROPFIND" => {
            if path == "/" && s.redirect_root {
                return (StatusCode::MOVED_PERMANENTLY, [("location", "/root/")]).into_response();
            }
            if path == "/" || path == "/root/" {
                return multistatus(&[ok_propstat(
                    path,
                    &format!(
                        "<current-user-principal><href>{PRINCIPAL}</href></current-user-principal>"
                    ),
                )]);
            }
            if path == PRINCIPAL {
                let origin = s
                    .home_origin
                    .clone()
                    .unwrap_or_else(|| format!("http://{host}"));
                return multistatus(&[ok_propstat(
                    PRINCIPAL,
                    &format!(
                        "<calendar-home-set xmlns=\"urn:ietf:params:xml:ns:caldav\"><href xmlns=\"DAV:\">{origin}{HOME}</href></calendar-home-set><calendar-user-address-set xmlns=\"urn:ietf:params:xml:ns:caldav\"><href xmlns=\"DAV:\">/1234567890/principal</href><href xmlns=\"DAV:\">urn:uuid:1234567890</href><href xmlns=\"DAV:\" preferred=\"1\">mailto:{EMAIL}</href><href xmlns=\"DAV:\">mailto:user.alias@example.org</href></calendar-user-address-set>"
                    ),
                )]);
            }
            if path == HOME {
                let mut out = vec![ok_propstat(
                    HOME,
                    "<resourcetype><collection/></resourcetype><supported-calendar-component-set xmlns=\"urn:ietf:params:xml:ns:caldav\"><comp name='VEVENT'/></supported-calendar-component-set>",
                )];
                for (slug, c) in &s.calendars {
                    out.push(collection_xml(slug, c));
                }
                return multistatus(&out);
            }
            if let Some(cal) = calendar_of_collection(&s, path) {
                let c = &s.calendars[&cal];
                let mut out = vec![collection_xml(&cal, c)];
                for (name, r) in &c.resources {
                    out.push(ok_propstat(
                        &format!("{HOME}{cal}/{name}"),
                        &format!("<getetag>{}</getetag>", xml_escape(&r.etag)),
                    ));
                }
                return multistatus(&out);
            }
            status(404)
        }
        "REPORT" => {
            let Some(cal) = calendar_of_collection(&s, path) else {
                return status(404);
            };
            let out: Vec<String> = s.calendars[&cal]
                .resources
                .iter()
                .map(|(name, r)| {
                    ok_propstat(
                        &format!("{HOME}{cal}/{name}"),
                        &format!(
                            "<getetag>{}</getetag><calendar-data xmlns=\"urn:ietf:params:xml:ns:caldav\"><![CDATA[{}]]></calendar-data>",
                            xml_escape(&r.etag),
                            r.body
                        ),
                    )
                })
                .collect();
            multistatus(&out)
        }
        "GET" => {
            let Some((cal, name)) = split_resource(path) else {
                return status(404);
            };
            match s.resource(&cal, &name) {
                Some(r) => (
                    StatusCode::OK,
                    [
                        ("content-type", "text/calendar; charset=utf-8".to_string()),
                        ("etag", r.etag),
                    ],
                    r.body,
                )
                    .into_response(),
                None => status(404),
            }
        }
        "PUT" => {
            let Some((cal, name)) = split_resource(path) else {
                return status(404);
            };
            let Some(c) = s.calendars.get(&cal) else {
                return status(404);
            };
            if !c.writable {
                return status(403);
            }
            let existing = c.resources.get(&name).map(|r| r.etag.clone());
            if header("if-none-match") == Some("*") && existing.is_some() {
                return status(412);
            }
            if let Some(want) = header("if-match")
                && existing.as_deref() != Some(want)
            {
                return status(412);
            }
            let created = existing.is_none();
            let text = String::from_utf8_lossy(body).into_owned();
            let etag = s.put(&cal, &name, &text);
            let code = if created {
                StatusCode::CREATED
            } else {
                StatusCode::NO_CONTENT
            };
            (code, [("etag", etag)]).into_response()
        }
        "DELETE" => {
            if let Some(cal) = calendar_of_collection(&s, path) {
                if !s.calendars[&cal].writable {
                    return status(403);
                }
                s.calendars.remove(&cal);
                return status(204);
            }
            let Some((cal, name)) = split_resource(path) else {
                return status(404);
            };
            let Some(existing) = s.resource(&cal, &name) else {
                return status(404);
            };
            if let Some(want) = header("if-match")
                && want != existing.etag
            {
                return status(412);
            }
            if !s.calendars[&cal].writable {
                return status(403);
            }
            s.calendars.get_mut(&cal).unwrap().resources.remove(&name);
            status(204)
        }
        "MKCALENDAR" => {
            let Some(slug) = path
                .strip_prefix(HOME)
                .and_then(|rest| rest.strip_suffix('/'))
                .filter(|slug| !slug.is_empty() && !slug.contains('/'))
            else {
                return status(403);
            };
            if s.calendars.contains_key(slug) {
                return status(405);
            }
            let text = String::from_utf8_lossy(body).into_owned();
            let components = if text.contains(r#"name="VEVENT""#) {
                vec!["VEVENT"]
            } else {
                Vec::new()
            };
            let name = element(&text, "displayname").unwrap_or_default();
            let calendar = FakeCalendar {
                color: element(&text, "calendar-color"),
                components,
                ..FakeCalendar::events(&name, true)
            };
            s.calendars.insert(slug.to_string(), calendar);
            status(201)
        }
        "PROPPATCH" => {
            let Some(cal) = calendar_of_collection(&s, path) else {
                return status(404);
            };
            if !s.calendars[&cal].writable {
                return status(403);
            }
            let href = format!("{HOME}{cal}/");
            if s.ignore_proppatch {
                return multistatus(&[format!(
                    "<response><href>{href}</href><propstat><prop><displayname/></prop><status>HTTP/1.1 403 Forbidden</status></propstat></response>"
                )]);
            }
            let text = String::from_utf8_lossy(body).into_owned();
            let c = s.calendars.get_mut(&cal).unwrap();
            if let Some(name) = element(&text, "displayname") {
                c.name = name;
            }
            if let Some(color) = element(&text, "calendar-color") {
                c.color = Some(color);
            }
            multistatus(&[ok_propstat(
                &href,
                "<displayname/><calendar-color xmlns=\"http://apple.com/ns/ical/\"/>",
            )])
        }
        _ => status(405),
    }
}

fn calendar_of_collection(s: &FakeState, path: &str) -> Option<String> {
    let slug = path.strip_prefix(HOME)?.strip_suffix('/')?;
    s.calendars.contains_key(slug).then(|| slug.to_string())
}

fn split_resource(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix(HOME)?;
    let (cal, name) = rest.split_once('/')?;
    (!name.is_empty() && !name.contains('/')).then(|| (cal.to_string(), name.to_string()))
}

/// The unescaped text of the first `<prefix:local>` element of a request body.
fn element(body: &str, local: &str) -> Option<String> {
    let open = format!(":{local}>");
    let start = body.find(&open)? + open.len();
    let end = start + body[start..].find("</")?;
    Some(
        body[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}
