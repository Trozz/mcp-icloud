//! Principal -> calendar home -> calendars, cached in memory, plus `event_id` validation.

use crate::caldav::client::CaldavClient;
use crate::caldav::xml::{propfind_calendars, propfind_home};
use crate::error::{CalError, Result};
use crate::ical::model::MyAddresses;
use mcp_icloud_dav::xml::{CALDAV, DavResponse, propfind_principal};
use std::sync::Arc;
use url::Url;

/// One calendar that holds events.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarInfo {
    /// The collection's href path, e.g. `/1234567890/calendars/home/`. Used as the calendar id.
    pub id: String,
    pub url: Url,
    pub name: String,
    pub color: Option<String>,
    pub writable: bool,
}

/// What discovery found for the account.
#[derive(Debug, Clone)]
pub struct Account {
    pub home: Url,
    pub calendars: Vec<CalendarInfo>,
    pub me: MyAddresses,
}

/// A collection that holds events: a calendar, supporting VEVENT, and not a scheduling inbox
/// or outbox. The home collection itself, notification and VTODO-only lists are excluded.
pub fn is_event_calendar(r: &DavResponse) -> bool {
    r.has_type(CALDAV, "calendar")
        && !r.has_type(CALDAV, "schedule-inbox")
        && !r.has_type(CALDAV, "schedule-outbox")
        && r.props.components.iter().any(|c| c == "VEVENT")
}

fn writable(r: &DavResponse) -> bool {
    r.props
        .privileges
        .iter()
        .any(|p| p == "write" || p == "write-content" || p == "all")
}

fn last_segment(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn join(base: &Url, href: &str) -> Result<Url> {
    base.join(href)
        .map_err(|e| CalError::Upstream(format!("invalid href {href:?} from the server: {e}")))
}

/// Raw input checks shared by calendar ids and event ids: no dot segments (they would be
/// normalized away by URL parsing), no encoded separators or dots, no backslashes.
fn reject_traversal(raw: &str, what: &str) -> Result<()> {
    let lower = raw.to_ascii_lowercase();
    let encoded = ["%2f", "%5c", "%2e"].iter().any(|e| lower.contains(e));
    let dots = raw
        .split(['/', '?', '#'])
        .any(|seg| seg == "." || seg == "..");
    if encoded || dots || raw.contains('\\') {
        return Err(CalError::invalid(format!(
            "{what} {raw:?} is not a valid id; use one returned by list_calendars or list_events"
        )));
    }
    Ok(())
}

impl Account {
    /// Resolves an absolute URL or a path against the calendar home, requiring the same origin.
    fn resolve(&self, raw: &str, what: &str) -> Result<Url> {
        let raw = raw.trim();
        reject_traversal(raw, what)?;
        let url = self
            .home
            .join(raw)
            .map_err(|_| CalError::invalid(format!("{what} {raw:?} is not a valid id")))?;
        if url.origin() != self.home.origin()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
        {
            return Err(CalError::invalid(format!(
                "{what} {raw:?} is not on this account's calendar server"
            )));
        }
        Ok(url)
    }

    /// A calendar by id (href path or absolute URL) or by display name (case-insensitive).
    pub fn calendar(&self, key: &str) -> Result<&CalendarInfo> {
        let key = key.trim();
        if key.contains('/') {
            let mut url = self.resolve(key, "calendar")?;
            if !url.path().ends_with('/') {
                url.set_path(&format!("{}/", url.path()));
            }
            return self
                .calendars
                .iter()
                .find(|c| c.url.path() == url.path())
                .ok_or_else(|| CalError::NotFound(format!("calendar {key}")));
        }
        let matches: Vec<&CalendarInfo> = self
            .calendars
            .iter()
            .filter(|c| c.name.eq_ignore_ascii_case(key))
            .collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => Err(CalError::NotFound(format!(
                "calendar {key:?}; use list_calendars for names and ids"
            ))),
            many => Err(CalError::invalid(format!(
                "more than one calendar is named {key:?}; use one of these ids: {}",
                many.iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// The calendar-home URL of an event resource and the calendar it belongs to. The event must
    /// be a direct child of a discovered calendar.
    pub fn resolve_event(&self, event_id: &str) -> Result<(Url, &CalendarInfo)> {
        let url = self.resolve(event_id, "event_id")?;
        let path = url.path();
        let cal = self
            .calendars
            .iter()
            .find(|c| {
                path.strip_prefix(c.url.path())
                    .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
            })
            .ok_or_else(|| {
                CalError::invalid(format!(
                    "event_id {event_id:?} is not an event in one of this account's calendars"
                ))
            })?;
        Ok((url, cal))
    }

    /// The event id for a resource URL: its href path.
    pub fn event_id(url: &Url) -> String {
        url.path().to_string()
    }

    /// URL for a new resource named after the UID.
    pub fn new_event_url(cal: &CalendarInfo, uid: &str) -> Result<Url> {
        let safe: String = uid
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        join(&cal.url, &format!("{safe}.ics"))
    }
}

/// Runs discovery from the client's base URL. `email` (the login) is always treated as mine.
pub async fn discover(client: &CaldavClient, email: &str) -> Result<Account> {
    let base = client.base().clone();
    let (answered, principal) = client.propfind(&base, 0, propfind_principal()).await?;
    let href = principal
        .iter()
        .find_map(|r| r.props.current_user_principal.clone())
        .ok_or_else(|| CalError::Upstream("the server did not name a principal".into()))?;
    let principal_url = join(&answered, &href)?;

    let (answered, home) = client.propfind(&principal_url, 0, propfind_home()).await?;
    let props = home
        .iter()
        .find(|r| r.props.calendar_home_set.is_some())
        .map(|r| r.props.clone())
        .ok_or_else(|| CalError::Upstream("the server did not name a calendar home".into()))?;
    let home_url = join(
        &answered,
        props.calendar_home_set.as_deref().unwrap_or_default(),
    )?;
    let mut addresses = props.address_set.clone();
    addresses.push(format!("mailto:{email}"));
    let me = MyAddresses::new(addresses);

    let (home_url, listing) = client.propfind(&home_url, 1, propfind_calendars()).await?;
    let mut calendars = Vec::new();
    for r in listing.iter().filter(|r| is_event_calendar(r)) {
        let url = join(&home_url, &r.href)?;
        let id = url.path().to_string();
        calendars.push(CalendarInfo {
            name: r
                .props
                .displayname
                .clone()
                .unwrap_or_else(|| last_segment(&id)),
            id,
            url,
            color: r.props.color.clone(),
            writable: writable(r),
        });
    }
    calendars.sort_by_key(|a| a.name.to_lowercase());
    tracing::info!(calendars = calendars.len(), "calendar discovery complete");
    Ok(Account {
        home: home_url,
        calendars,
        me,
    })
}

/// Discovery results shared by all sessions. The first caller runs discovery; later callers
/// reuse it until `refresh` or `invalidate`.
pub struct Directory {
    client: Arc<CaldavClient>,
    email: String,
    cached: tokio::sync::Mutex<Option<Arc<Account>>>,
}

impl Directory {
    pub fn new(client: Arc<CaldavClient>, email: String) -> Self {
        Directory {
            client,
            email,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    pub fn client(&self) -> &CaldavClient {
        &self.client
    }

    pub async fn account(&self) -> Result<Arc<Account>> {
        let mut cached = self.cached.lock().await;
        if let Some(a) = cached.as_ref() {
            return Ok(a.clone());
        }
        let a = Arc::new(discover(&self.client, &self.email).await?);
        *cached = Some(a.clone());
        Ok(a)
    }

    pub async fn refresh(&self) -> Result<Arc<Account>> {
        self.invalidate().await;
        self.account().await
    }

    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_icloud_dav::xml::parse_multistatus;

    const CALENDARS: &str = include_str!("../../tests/fixtures/caldav-calendars.xml");

    fn account() -> Account {
        let home = Url::parse("https://p99-caldav.icloud.com:443/1234567890/calendars/").unwrap();
        let cal = |path: &str, name: &str| CalendarInfo {
            id: path.into(),
            url: home.join(path).unwrap(),
            name: name.into(),
            color: None,
            writable: true,
        };
        Account {
            calendars: vec![
                cal("/1234567890/calendars/home/", "Home"),
                cal("/1234567890/calendars/work/", "Work"),
                cal("/1234567890/calendars/work-2/", "work"),
            ],
            home,
            me: MyAddresses::new(["mailto:user@example.com"]),
        }
    }

    #[test]
    fn only_event_calendars_survive_the_filter() {
        let r = parse_multistatus(CALENDARS).unwrap();
        let kept: Vec<&str> = r
            .iter()
            .filter(|x| is_event_calendar(x))
            .map(|x| x.href.as_str())
            .collect();
        assert_eq!(
            kept,
            [
                "/1234567890/calendars/home/",
                "/1234567890/calendars/holidays/"
            ]
        );
        let holidays = r.iter().find(|x| x.href.ends_with("/holidays/")).unwrap();
        assert!(!writable(holidays));
        let home = r.iter().find(|x| x.href.ends_with("/home/")).unwrap();
        assert!(writable(home));
    }

    #[test]
    fn home_url_drops_the_default_port() {
        assert_eq!(
            account().home.as_str(),
            "https://p99-caldav.icloud.com/1234567890/calendars/"
        );
    }

    #[test]
    fn event_ids_are_accepted_as_paths_and_absolute_urls() {
        let a = account();
        for id in [
            "/1234567890/calendars/home/abc.ics",
            "https://p99-caldav.icloud.com/1234567890/calendars/home/abc.ics",
            "https://p99-caldav.icloud.com:443/1234567890/calendars/home/abc.ics",
            "  /1234567890/calendars/home/abc.ics ",
        ] {
            let (url, cal) = a.resolve_event(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(
                url.as_str(),
                "https://p99-caldav.icloud.com/1234567890/calendars/home/abc.ics"
            );
            assert_eq!(cal.name, "Home");
            assert_eq!(
                Account::event_id(&url),
                "/1234567890/calendars/home/abc.ics"
            );
        }
    }

    #[test]
    fn hostile_event_ids_are_rejected() {
        let a = account();
        for id in [
            "https://evil.example.com/1234567890/calendars/home/abc.ics",
            "http://p99-caldav.icloud.com/1234567890/calendars/home/abc.ics",
            "https://p98-caldav.icloud.com/1234567890/calendars/home/abc.ics",
            "https://p99-caldav.icloud.com:8443/1234567890/calendars/home/abc.ics",
            "//evil.example.com/1234567890/calendars/home/abc.ics",
            "/1234567890/calendars/home/../work/abc.ics",
            "/1234567890/calendars/home/./abc.ics",
            "/1234567890/calendars/home/%2e%2e/work/abc.ics",
            "/1234567890/calendars/home/%2E%2E/abc.ics",
            "/1234567890/calendars/home%2Fabc.ics",
            "/1234567890/calendars/home/sub/abc.ics",
            "/1234567890/calendars/home/",
            "/1234567890/calendars/other/abc.ics",
            "/1234567890/principal/",
            "/1234567890/calendars/home/abc.ics?x=1",
            "/1234567890/calendars/home/abc.ics#frag",
            "\\1234567890\\calendars\\home\\abc.ics",
            "https://user@p99-caldav.icloud.com/1234567890/calendars/home/abc.ics",
        ] {
            let e = a.resolve_event(id).unwrap_err();
            assert_eq!(e.kind(), "validation", "{id}");
        }
    }

    #[test]
    fn calendars_are_found_by_id_or_unique_name() {
        let a = account();
        assert_eq!(
            a.calendar("home").unwrap().id,
            "/1234567890/calendars/home/"
        );
        assert_eq!(
            a.calendar("/1234567890/calendars/home").unwrap().name,
            "Home"
        );
        assert_eq!(
            a.calendar("https://p99-caldav.icloud.com/1234567890/calendars/work-2/")
                .unwrap()
                .name,
            "work"
        );
        let e = a.calendar("WORK").unwrap_err();
        assert_eq!(e.kind(), "validation");
        assert!(
            e.to_string().contains("/1234567890/calendars/work-2/"),
            "{e}"
        );
        assert_eq!(a.calendar("Birthdays").unwrap_err().kind(), "not_found");
        assert_eq!(
            a.calendar("/1234567890/calendars/../principal/")
                .unwrap_err()
                .kind(),
            "validation"
        );
    }

    #[test]
    fn new_event_urls_are_named_after_a_safe_uid() {
        let a = account();
        let cal = a.calendar("Home").unwrap();
        assert_eq!(
            Account::new_event_url(cal, "0A1B-2c3d").unwrap().path(),
            "/1234567890/calendars/home/0A1B-2c3d.ics"
        );
        assert_eq!(
            Account::new_event_url(cal, "a/b@c..d").unwrap().path(),
            "/1234567890/calendars/home/a_b_c__d.ics"
        );
    }
}
