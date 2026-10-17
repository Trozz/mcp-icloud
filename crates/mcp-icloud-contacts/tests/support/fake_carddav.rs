//! In-memory CardDAV server shaped like iCloud (probed 2026-10-17): a principal, an address book
//! home with `me-card`, one address book `card` with sync tokens and multiget, and iCloud's write
//! quirks: `403` (not `412`) for `If-None-Match: *` on an existing card, and `DELETE` that ignores
//! `If-Match` and answers `204` for a missing card. Cards are stored exactly as sent. Every
//! request is logged for assertions.
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use mcp_icloud_dav::client::{DavClient, DavSettings, Protocol};
use secrecy::SecretString;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use url::Url;

pub const PRINCIPAL: &str = "/1234567890/principal/";
pub const HOME: &str = "/1234567890/carddavhome/";
pub const BOOK: &str = "/1234567890/carddavhome/card/";
pub const EMAIL: &str = "user@example.com";
pub const PASSWORD: &str = "app-pass";
/// `Basic base64("user@example.com:app-pass")`.
pub const AUTH: &str = "Basic dXNlckBleGFtcGxlLmNvbTphcHAtcGFzcw==";
const TOKEN_PREFIX: &str = "http://example.com/sync/";

#[derive(Debug, Clone)]
pub struct StoredCard {
    pub etag: String,
    pub body: String,
    /// The change number when it was last written.
    pub changed: u64,
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
    /// Cards by file name (the last path segment, which need not be the UID).
    pub cards: BTreeMap<String, StoredCard>,
    /// Deleted file names and the change number of their deletion.
    pub deleted: BTreeMap<String, u64>,
    /// File name of the owner's card, announced as `me-card`.
    pub me: Option<String>,
    /// Extra address books listed before `card` (by slug).
    pub extra_books: Vec<String>,
    /// The latest change number; tokens name the change they were issued at.
    pub version: u64,
    /// Tokens issued before this change number are refused with `412 valid-sync-token`.
    pub oldest_token: u64,
    pub max_resource_size: u64,
    pub log: Vec<Logged>,
    /// Next failures to inject: (method, status). The first entry whose method matches is used.
    pub fail: VecDeque<(&'static str, u16)>,
    /// Answer PROPFIND on `/` with a redirect to `/root/`, which then serves the principal.
    pub redirect_root: bool,
}

impl FakeState {
    /// An empty `card` address book, like a new iCloud account.
    pub fn icloud_like() -> Self {
        FakeState {
            cards: BTreeMap::new(),
            deleted: BTreeMap::new(),
            me: None,
            extra_books: Vec::new(),
            version: 1,
            oldest_token: 0,
            max_resource_size: 563_200,
            log: Vec::new(),
            fail: VecDeque::new(),
            redirect_root: false,
        }
    }

    /// Stores (or replaces) a card as another device would; returns its ETag. LF line endings are
    /// turned into CRLF, as iCloud clients send them.
    pub fn put(&mut self, name: &str, vcf: &str) -> String {
        let body = vcf.replace("\r\n", "\n").replace('\n', "\r\n");
        self.store(name, body)
    }

    fn store(&mut self, name: &str, body: String) -> String {
        self.version += 1;
        let etag = format!("\"etag-{}\"", self.version);
        self.deleted.remove(name);
        self.cards.insert(
            name.into(),
            StoredCard {
                etag: etag.clone(),
                body,
                changed: self.version,
            },
        );
        etag
    }

    /// Deletes a card as another device would.
    pub fn remove(&mut self, name: &str) {
        if self.cards.remove(name).is_some() {
            self.version += 1;
            self.deleted.insert(name.into(), self.version);
        }
    }

    pub fn card(&self, name: &str) -> Option<StoredCard> {
        self.cards.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.cards.keys().cloned().collect()
    }

    pub fn count(&self, method: &str) -> usize {
        self.log.iter().filter(|l| l.method == method).count()
    }

    /// REPORTs whose body names `what` (`sync-collection` or `addressbook-multiget`).
    pub fn reports(&self, what: &str) -> Vec<Logged> {
        self.log
            .iter()
            .filter(|l| l.method == "REPORT" && l.body.contains(what))
            .cloned()
            .collect()
    }

    /// Makes every token issued so far invalid, as when iCloud expires them.
    pub fn expire_tokens(&mut self) {
        self.oldest_token = self.version + 1;
    }

    fn token(&self) -> String {
        format!("{TOKEN_PREFIX}{}", self.version)
    }
}

#[derive(Clone)]
pub struct FakeCardDav {
    pub url: Url,
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeCardDav {
    pub async fn start(state: FakeState) -> FakeCardDav {
        let state = Arc::new(Mutex::new(state));
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FakeCardDav {
            url: Url::parse(&format!("http://{addr}/")).unwrap(),
            state,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    /// A CardDAV client for this server with the right credentials.
    pub fn client(&self) -> Arc<DavClient> {
        self.client_with(PASSWORD)
    }

    pub fn client_with(&self, password: &str) -> Arc<DavClient> {
        Arc::new(
            DavClient::new(DavSettings::new(
                Protocol::CardDav,
                self.url.clone(),
                EMAIL.into(),
                SecretString::from(password.to_string()),
                "mcp-icloud-contacts/test".into(),
            ))
            .unwrap(),
        )
    }
}

fn multistatus(responses: &[String], token: Option<String>) -> Response {
    let token = token
        .map(|t| format!("<sync-token>{t}</sync-token>"))
        .unwrap_or_default();
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<multistatus xmlns=\"DAV:\">\n{}\n{token}</multistatus>",
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

fn missing(href: &str) -> String {
    format!("<response><href>{href}</href><status>HTTP/1.1 404 Not Found</status></response>")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The text of every `<name>` element in a request body, whatever its namespace prefix. A
/// self-closing element (`<d:sync-token/>`) has empty text.
fn elements(body: &str, name: &str) -> Vec<String> {
    body.split('<')
        .filter_map(|piece| {
            let (tag, text) = piece.split_once('>')?;
            if tag.starts_with('/') || tag.starts_with('?') {
                return None;
            }
            let local = tag
                .trim_end_matches('/')
                .split_whitespace()
                .next()?
                .rsplit(':')
                .next()?;
            if local != name {
                return None;
            }
            if tag.ends_with('/') {
                return Some(String::new());
            }
            Some(xml_unescape(text.trim()))
        })
        .collect()
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
    let body = String::from_utf8_lossy(&body).into_owned();
    let mut s = state.lock().unwrap();
    s.log.push(Logged {
        method: method.to_string(),
        path: path.clone(),
        authorized: header("authorization").as_deref() == Some(AUTH),
        depth: header("depth"),
        if_match: header("if-match"),
        if_none_match: header("if-none-match"),
        body: body.clone(),
    });
    if header("authorization").as_deref() != Some(AUTH) {
        return status(401);
    }
    if let Some(i) = s.fail.iter().position(|(m, _)| *m == method.as_str()) {
        let (_, code) = s.fail.remove(i).unwrap();
        return status(code);
    }
    let host = header("host").unwrap_or_default();
    match method.as_str() {
        "PROPFIND" => propfind(&s, &path, &host),
        "REPORT" if path == BOOK && body.contains("sync-collection") => sync(&s, &body),
        "REPORT" if path == BOOK && body.contains("addressbook-multiget") => multiget(&s, &body),
        "REPORT" => status(404),
        "GET" => match card_name(&path).and_then(|n| s.card(&n)) {
            Some(c) => (
                StatusCode::OK,
                [
                    ("content-type", "text/plain; charset=UTF-8".to_string()),
                    ("etag", c.etag),
                ],
                c.body,
            )
                .into_response(),
            None => status(404),
        },
        "PUT" => {
            let Some(name) = card_name(&path) else {
                return status(404);
            };
            let existing = s.card(&name).map(|c| c.etag);
            if header("if-none-match").as_deref() == Some("*") && existing.is_some() {
                return status(403);
            }
            if let Some(want) = header("if-match")
                && existing.as_deref() != Some(want.as_str())
            {
                return status(412);
            }
            if body.len() as u64 > s.max_resource_size {
                return status(413);
            }
            let created = existing.is_none();
            let etag = s.store(&name, body);
            let code = if created {
                StatusCode::CREATED
            } else {
                StatusCode::NO_CONTENT
            };
            (code, [("etag", etag)]).into_response()
        }
        "DELETE" => {
            if let Some(name) = card_name(&path) {
                s.remove(&name);
            }
            status(204)
        }
        _ => status(405),
    }
}

fn propfind(s: &FakeState, path: &str, host: &str) -> Response {
    if path == "/" && s.redirect_root {
        return (StatusCode::MOVED_PERMANENTLY, [("location", "/root/")]).into_response();
    }
    if path == "/" || path == "/root/" {
        return multistatus(
            &[ok_propstat(
                path,
                &format!(
                    "<current-user-principal><href>{PRINCIPAL}</href></current-user-principal>"
                ),
            )],
            None,
        );
    }
    if path == PRINCIPAL {
        return multistatus(
            &[ok_propstat(
                PRINCIPAL,
                &format!(
                    "<addressbook-home-set xmlns=\"urn:ietf:params:xml:ns:carddav\"><href xmlns=\"DAV:\">http://{host}{HOME}</href></addressbook-home-set>"
                ),
            )],
            None,
        );
    }
    if path == HOME {
        let me = s
            .me
            .as_ref()
            .map(|m| {
                format!(
                    "<me-card xmlns=\"http://calendarserver.org/ns/\"><href xmlns=\"DAV:\">{BOOK}{m}</href></me-card>"
                )
            })
            .unwrap_or_default();
        let mut out = vec![ok_propstat(
            HOME,
            &format!("<resourcetype><collection/></resourcetype>{me}"),
        )];
        let book = |slug: &str| {
            ok_propstat(
                &format!("{HOME}{slug}/"),
                &format!(
                    "<resourcetype><collection/><addressbook xmlns=\"urn:ietf:params:xml:ns:carddav\"/></resourcetype><displayname>{slug}</displayname><sync-token>{}</sync-token><max-resource-size xmlns=\"urn:ietf:params:xml:ns:carddav\">{}</max-resource-size>",
                    s.token(),
                    s.max_resource_size
                ),
            )
        };
        for slug in &s.extra_books {
            out.push(book(slug));
        }
        out.push(book("card"));
        return multistatus(&out, None);
    }
    status(404)
}

fn sync(s: &FakeState, body: &str) -> Response {
    let since = match elements(body, "sync-token").first() {
        None => None,
        Some(t) if t.is_empty() => None,
        Some(t) => match t
            .strip_prefix(TOKEN_PREFIX)
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| *n >= s.oldest_token)
        {
            Some(n) => Some(n),
            None => {
                return (
                    StatusCode::PRECONDITION_FAILED,
                    [("content-type", "application/xml; charset=UTF-8")],
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<error xmlns=\"DAV:\"><valid-sync-token/></error>",
                )
                    .into_response();
            }
        },
    };
    let mut out = Vec::new();
    if since.is_none() {
        // iCloud lists the collection itself (without a trailing slash) in a full sync.
        out.push(ok_propstat(
            BOOK.trim_end_matches('/'),
            "<getetag>\"collection\"</getetag>",
        ));
    }
    for (name, c) in &s.cards {
        if since.is_none_or(|n| c.changed > n) {
            out.push(ok_propstat(
                &format!("{BOOK}{name}"),
                &format!("<getetag>{}</getetag>", xml_escape(&c.etag)),
            ));
        }
    }
    if let Some(n) = since {
        for (name, at) in &s.deleted {
            if *at > n {
                out.push(missing(&format!("{BOOK}{name}")));
            }
        }
    }
    multistatus(&out, Some(s.token()))
}

fn multiget(s: &FakeState, body: &str) -> Response {
    let out: Vec<String> = elements(body, "href")
        .iter()
        .map(|href| match card_name(href).and_then(|n| s.card(&n)) {
            Some(c) => ok_propstat(
                href,
                &format!(
                    "<getetag>{}</getetag><address-data xmlns=\"urn:ietf:params:xml:ns:carddav\">{}</address-data>",
                    xml_escape(&c.etag),
                    xml_escape(&c.body)
                ),
            ),
            None => missing(href),
        })
        .collect();
    multistatus(&out, None)
}

fn card_name(path: &str) -> Option<String> {
    let name = path.strip_prefix(BOOK)?;
    (!name.is_empty() && !name.contains('/')).then(|| name.to_string())
}
