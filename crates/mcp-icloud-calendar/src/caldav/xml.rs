//! WebDAV/CalDAV request bodies and `multistatus` parsing.
//!
//! Elements are matched by namespace plus local name (iCloud declares namespaces per element).
//! Only properties from a `propstat` with a 2xx status are kept, so the 404 blocks iCloud sends
//! for unsupported properties are ignored.

use crate::error::{CalError, Result};
use chrono::{DateTime, Utc};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CALSERVER: &str = "http://calendarserver.org/ns/";
pub const APPLE: &str = "http://apple.com/ns/ical/";

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;

pub fn propfind_principal() -> String {
    format!(
        r#"{HEAD}<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#
    )
}

pub fn propfind_home() -> String {
    format!(
        r#"{HEAD}<d:propfind xmlns:d="DAV:" xmlns:c="{CALDAV}"><d:prop><c:calendar-home-set/><c:calendar-user-address-set/></d:prop></d:propfind>"#
    )
}

pub fn propfind_calendars() -> String {
    format!(
        r#"{HEAD}<d:propfind xmlns:d="DAV:" xmlns:c="{CALDAV}" xmlns:cs="{CALSERVER}" xmlns:a="{APPLE}"><d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/><d:current-user-privilege-set/><cs:getctag/></d:prop></d:propfind>"#
    )
}

/// `calendar-query` for every VEVENT resource overlapping `[start, end)`.
pub fn calendar_query(start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    let fmt = "%Y%m%dT%H%M%SZ";
    format!(
        r#"{HEAD}<c:calendar-query xmlns:d="DAV:" xmlns:c="{CALDAV}"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{}" end="{}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#,
        start.format(fmt),
        end.format(fmt)
    )
}

/// The properties we read, from every 2xx `propstat` of one `response`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props {
    pub current_user_principal: Option<String>,
    pub calendar_home_set: Option<String>,
    pub address_set: Vec<String>,
    pub displayname: Option<String>,
    /// Resource types in Clark notation, e.g. `{urn:ietf:params:xml:ns:caldav}calendar`.
    pub resource_types: Vec<String>,
    /// Supported calendar components, e.g. `VEVENT`.
    pub components: Vec<String>,
    /// Privilege local names, e.g. `write`, `write-content`.
    pub privileges: Vec<String>,
    pub color: Option<String>,
    pub ctag: Option<String>,
    pub etag: Option<String>,
    pub calendar_data: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DavResponse {
    pub href: String,
    /// Status of the whole response (`<response><status>`), when present.
    pub status: Option<u16>,
    pub props: Props,
}

impl DavResponse {
    pub fn has_type(&self, ns: &str, local: &str) -> bool {
        self.props.resource_types.contains(&clark(ns, local))
    }
}

fn clark(ns: &str, local: &str) -> String {
    format!("{{{ns}}}{local}")
}

/// "HTTP/1.1 404 Not Found" -> 404.
fn status_code(s: &str) -> Option<u16> {
    s.split_whitespace().nth(1)?.parse().ok()
}

#[derive(Default)]
struct Partial {
    href: Option<String>,
    status: Option<u16>,
    props: Props,
    stat_props: Props,
    stat_status: Option<u16>,
}

fn merge(into: &mut Props, from: Props) {
    let Props {
        current_user_principal,
        calendar_home_set,
        address_set,
        displayname,
        resource_types,
        components,
        privileges,
        color,
        ctag,
        etag,
        calendar_data,
    } = from;
    into.current_user_principal = into
        .current_user_principal
        .take()
        .or(current_user_principal);
    into.calendar_home_set = into.calendar_home_set.take().or(calendar_home_set);
    into.address_set.extend(address_set);
    into.displayname = into.displayname.take().or(displayname);
    into.resource_types.extend(resource_types);
    into.components.extend(components);
    into.privileges.extend(privileges);
    into.color = into.color.take().or(color);
    into.ctag = into.ctag.take().or(ctag);
    into.etag = into.etag.take().or(etag);
    into.calendar_data = into.calendar_data.take().or(calendar_data);
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn bad(e: impl std::fmt::Display) -> CalError {
    CalError::Upstream(format!("unreadable multistatus response: {e}"))
}

fn namespace(r: &ResolveResult) -> String {
    match r {
        ResolveResult::Bound(ns) => ns.as_ref().to_string(),
        _ => String::new(),
    }
}

fn comp_name(e: &BytesStart) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == "name")
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Parses a `207 Multi-Status` body into one entry per `response`.
pub fn parse_multistatus(xml: &str) -> Result<Vec<DavResponse>> {
    let mut reader = NsReader::from_str(xml);
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut text = String::new();
    let mut out = Vec::new();
    let mut cur: Option<Partial> = None;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(bad)?;
        let ns = namespace(&ns);
        match event {
            Event::Start(e) => {
                let local = e.local_name().as_ref().to_string();
                on_open(&stack, &ns, &local, &e, &mut cur);
                stack.push((ns, local));
                text.clear();
            }
            Event::Empty(e) => {
                let local = e.local_name().as_ref().to_string();
                on_open(&stack, &ns, &local, &e, &mut cur);
                stack.push((ns, local));
                text.clear();
                on_close(&mut stack, &mut text, &mut cur, &mut out);
            }
            Event::End(_) => on_close(&mut stack, &mut text, &mut cur, &mut out),
            Event::Text(t) => text.push_str(&t.into_inner()),
            Event::CData(t) => text.push_str(&t.into_inner()),
            Event::GeneralRef(r) => {
                let entity = format!("&{};", r.as_ref());
                let resolved = quick_xml::escape::unescape(&entity).map_err(bad)?;
                text.push_str(&resolved);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn parent_is(stack: &[(String, String)], depth_from_top: usize, ns: &str, local: &str) -> bool {
    stack
        .len()
        .checked_sub(depth_from_top + 1)
        .and_then(|i| stack.get(i))
        .is_some_and(|(n, l)| n == ns && l == local)
}

fn on_open(
    stack: &[(String, String)],
    ns: &str,
    local: &str,
    e: &BytesStart,
    cur: &mut Option<Partial>,
) {
    if ns == DAV && local == "response" {
        *cur = Some(Partial::default());
        return;
    }
    let Some(p) = cur.as_mut() else { return };
    if ns == DAV && local == "propstat" {
        p.stat_props = Props::default();
        p.stat_status = None;
        return;
    }
    if parent_is(stack, 0, DAV, "resourcetype") {
        p.stat_props.resource_types.push(clark(ns, local));
        return;
    }
    if ns == CALDAV
        && local == "comp"
        && parent_is(stack, 0, CALDAV, "supported-calendar-component-set")
    {
        if let Some(name) = comp_name(e) {
            p.stat_props.components.push(name.to_ascii_uppercase());
        }
        return;
    }
    if parent_is(stack, 0, DAV, "privilege") {
        p.stat_props.privileges.push(local.to_string());
    }
}

fn on_close(
    stack: &mut Vec<(String, String)>,
    text: &mut String,
    cur: &mut Option<Partial>,
    out: &mut Vec<DavResponse>,
) {
    let Some((ns, local)) = stack.pop() else {
        return;
    };
    let value = std::mem::take(text);
    let trimmed = value.trim().to_string();
    if ns == DAV && local == "response" {
        if let Some(p) = cur.take()
            && let Some(href) = p.href
        {
            out.push(DavResponse {
                href,
                status: p.status,
                props: p.props,
            });
        }
        return;
    }
    let Some(p) = cur.as_mut() else { return };
    let (ns, local) = (ns.as_str(), local.as_str());
    match (ns, local) {
        (DAV, "propstat") => {
            let stat = std::mem::take(&mut p.stat_props);
            if p.stat_status.is_some_and(|s| (200..300).contains(&s)) {
                merge(&mut p.props, stat);
            }
        }
        (DAV, "status") if parent_is(stack, 0, DAV, "propstat") => {
            p.stat_status = status_code(&trimmed);
        }
        (DAV, "status") if parent_is(stack, 0, DAV, "response") => {
            p.status = status_code(&trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, DAV, "response") => p.href = non_empty(trimmed),
        (DAV, "href") if parent_is(stack, 0, DAV, "current-user-principal") => {
            p.stat_props.current_user_principal = non_empty(trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, CALDAV, "calendar-home-set") => {
            p.stat_props.calendar_home_set = non_empty(trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, CALDAV, "calendar-user-address-set") => {
            if !trimmed.is_empty() {
                p.stat_props.address_set.push(trimmed);
            }
        }
        (DAV, "displayname") => p.stat_props.displayname = non_empty(trimmed),
        (DAV, "getetag") => p.stat_props.etag = non_empty(trimmed),
        (APPLE, "calendar-color") => p.stat_props.color = non_empty(trimmed),
        (CALSERVER, "getctag") => p.stat_props.ctag = non_empty(trimmed),
        (CALDAV, "calendar-data") => p.stat_props.calendar_data = non_empty(trimmed),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const PRINCIPAL: &str = include_str!("../../tests/fixtures/caldav-principal.xml");
    const HOME: &str = include_str!("../../tests/fixtures/caldav-home.xml");
    const CALENDARS: &str = include_str!("../../tests/fixtures/caldav-calendars.xml");
    const REPORT: &str = include_str!("../../tests/fixtures/caldav-report.xml");

    #[test]
    fn request_bodies_are_well_formed_and_namespaced() {
        for body in [
            propfind_principal(),
            propfind_home(),
            propfind_calendars(),
            calendar_query(
                Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
            ),
        ] {
            let mut r = NsReader::from_str(&body);
            loop {
                let (ns, ev) = r.read_resolved_event().expect("well-formed XML");
                if let Event::Start(_) | Event::Empty(_) = ev {
                    assert!(matches!(ns, ResolveResult::Bound(_)), "{body}");
                }
                if let Event::Eof = ev {
                    break;
                }
            }
        }
    }

    #[test]
    fn calendar_query_uses_utc_basic_format_range() {
        let q = calendar_query(
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 10, 8, 12, 30, 0).unwrap(),
        );
        assert!(
            q.contains(r#"<c:time-range start="20261001T000000Z" end="20261008T123000Z"/>"#),
            "{q}"
        );
        assert!(q.contains(r#"<c:comp-filter name="VEVENT">"#));
    }

    #[test]
    fn principal_href_is_read() {
        let r = parse_multistatus(PRINCIPAL).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].href, "/");
        assert_eq!(
            r[0].props.current_user_principal.as_deref(),
            Some("/1234567890/principal/")
        );
    }

    #[test]
    fn home_set_is_absolute_and_addresses_are_all_kept() {
        let r = parse_multistatus(HOME).unwrap();
        let p = &r[0].props;
        assert_eq!(
            p.calendar_home_set.as_deref(),
            Some("https://p99-caldav.icloud.com:443/1234567890/calendars/")
        );
        assert_eq!(
            p.address_set,
            [
                "/1234567890/principal",
                "urn:uuid:1234567890",
                "mailto:user@example.com",
                "mailto:user.alias@example.org"
            ]
        );
    }

    #[test]
    fn calendar_list_reads_types_components_privileges_and_ignores_404_propstats() {
        let r = parse_multistatus(CALENDARS).unwrap();
        assert_eq!(r.len(), 7);
        let home = r
            .iter()
            .find(|x| x.href == "/1234567890/calendars/home/")
            .unwrap();
        assert!(home.has_type(CALDAV, "calendar"));
        assert!(home.has_type(CALSERVER, "shared-owner"));
        assert_eq!(home.props.displayname.as_deref(), Some("Home & Family"));
        assert_eq!(home.props.color.as_deref(), Some("#999999FF"));
        assert_eq!(home.props.components, ["VEVENT"]);
        assert!(home.props.privileges.iter().any(|p| p == "write-content"));
        assert_eq!(home.props.ctag.as_deref(), Some("ctag-home"));

        let root = &r[0];
        assert!(!root.has_type(CALDAV, "calendar"));
        assert_eq!(root.props.color, None, "the 404 propstat must not leak in");
        assert_eq!(root.props.components, ["VEVENT", "VTODO", "VFREEBUSY"]);

        let inbox = r.iter().find(|x| x.href.ends_with("/inbox/")).unwrap();
        assert!(inbox.has_type(CALDAV, "schedule-inbox"));
        assert_eq!(inbox.props.displayname, None);
    }

    #[test]
    fn report_reads_cdata_and_entity_escaped_calendar_data() {
        let r = parse_multistatus(REPORT).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].props.etag.as_deref(), Some("\"mvcd0001\""));
        let data = r[0].props.calendar_data.as_deref().unwrap();
        assert!(data.starts_with("BEGIN:VCALENDAR"));
        assert!(data.contains("SUMMARY:Dentist <check-up> & clean"));
        assert!(data.ends_with("END:VCALENDAR"));

        assert_eq!(r[1].props.etag.as_deref(), Some("\"mvcd0002\""));
        let data = r[1].props.calendar_data.as_deref().unwrap();
        assert!(
            data.contains("SUMMARY:Fish & chips <Friday>\r\n"),
            "{data:?}"
        );

        assert_eq!(r[2].href, "/1234567890/calendars/home/gone.ics");
        assert_eq!(r[2].status, Some(404));
        assert_eq!(r[2].props.calendar_data, None);
    }

    #[test]
    fn report_calendar_data_parses_as_icalendar() {
        let r = parse_multistatus(REPORT).unwrap();
        for resp in &r[..2] {
            let cal =
                crate::ical::parse::parse_resource(resp.props.calendar_data.as_deref().unwrap())
                    .unwrap();
            assert!(crate::ical::parse::master(&cal).is_some());
        }
    }

    #[test]
    fn garbage_is_an_upstream_error() {
        let e = parse_multistatus("<multistatus xmlns=\"DAV:\"><response></nope>").unwrap_err();
        assert_eq!(e.kind(), "upstream");
    }

    #[test]
    fn empty_multistatus_is_empty() {
        assert!(
            parse_multistatus(r#"<?xml version="1.0"?><multistatus xmlns="DAV:"/>"#)
                .unwrap()
                .is_empty()
        );
    }
}
