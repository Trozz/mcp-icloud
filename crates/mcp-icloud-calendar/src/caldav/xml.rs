//! CalDAV request bodies used only by the calendar service. Parsing and the shared bodies live in
//! `mcp_icloud_dav::xml`.

use chrono::{DateTime, Utc};
use mcp_icloud_dav::xml::{APPLE, CALDAV, CALSERVER};
use quick_xml::escape::escape;

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;

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

/// `mkcalendar` (RFC 4791) for a calendar that holds events only, with its name and an optional
/// colour (`#RRGGBBAA`, Apple's namespace).
pub fn mkcalendar(name: &str, color: Option<&str>) -> String {
    format!(
        r#"{HEAD}<c:mkcalendar xmlns:d="DAV:" xmlns:c="{CALDAV}" xmlns:a="{APPLE}"><d:set><d:prop><d:displayname>{}</d:displayname>{}<c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"#,
        escape(name),
        color_prop(color)
    )
}

/// `propertyupdate` (RFC 4918 PROPPATCH) setting a calendar's name, colour or both.
pub fn propertyupdate(name: Option<&str>, color: Option<&str>) -> String {
    let name = name
        .map(|n| format!("<d:displayname>{}</d:displayname>", escape(n)))
        .unwrap_or_default();
    format!(
        r#"{HEAD}<d:propertyupdate xmlns:d="DAV:" xmlns:a="{APPLE}"><d:set><d:prop>{name}{}</d:prop></d:set></d:propertyupdate>"#,
        color_prop(color)
    )
}

fn color_prop(color: Option<&str>) -> String {
    color
        .map(|c| format!("<a:calendar-color>{}</a:calendar-color>", escape(c)))
        .unwrap_or_default()
}

/// Depth 1 PROPFIND for the members of a collection. The collection answers for itself too.
pub fn propfind_members() -> String {
    format!(
        r#"{HEAD}<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getetag/></d:prop></d:propfind>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mcp_icloud_dav::xml::{parse_multistatus, propfind_principal};
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    use quick_xml::reader::NsReader;

    const PRINCIPAL: &str = include_str!("../../tests/fixtures/caldav-principal.xml");
    const HOME: &str = include_str!("../../tests/fixtures/caldav-home.xml");
    const CALENDARS: &str = include_str!("../../tests/fixtures/caldav-calendars.xml");
    const REPORT: &str = include_str!("../../tests/fixtures/caldav-report.xml");

    #[test]
    fn request_bodies_are_well_formed_and_namespaced() {
        for body in [
            propfind_principal(),
            propfind_calendars(),
            calendar_query(
                Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
            ),
            mkcalendar("Kids & <School>", Some("#1BADF8FF")),
            mkcalendar("Home", None),
            propertyupdate(Some("Renamed"), Some("#000000FF")),
            propertyupdate(None, Some("#000000FF")),
            propfind_members(),
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
    fn mkcalendar_escapes_the_name_and_offers_only_events() {
        let b = mkcalendar("Kids & <School>", Some("#1BADF8FF"));
        assert!(b.contains("<c:mkcalendar "), "{b}");
        assert!(
            b.contains("<d:set><d:prop><d:displayname>Kids &amp; &lt;School&gt;</d:displayname>"),
            "{b}"
        );
        assert!(
            b.contains("<a:calendar-color>#1BADF8FF</a:calendar-color>"),
            "{b}"
        );
        assert!(
            b.contains(r#"<c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set>"#),
            "{b}"
        );
        assert!(b.contains(r#"xmlns:a="http://apple.com/ns/ical/""#), "{b}");
        assert!(!mkcalendar("Home", None).contains("calendar-color"));
    }

    #[test]
    fn propertyupdate_sets_only_the_given_properties() {
        let both = propertyupdate(Some("A & B"), Some("#00FF00FF"));
        assert!(
            both.contains("<d:propertyupdate ")
                && both.contains("<d:displayname>A &amp; B</d:displayname>")
                && both.contains("<a:calendar-color>#00FF00FF</a:calendar-color>"),
            "{both}"
        );
        let name = propertyupdate(Some("A"), None);
        assert!(!name.contains("calendar-color"), "{name}");
        let color = propertyupdate(None, Some("#00FF00FF"));
        assert!(!color.contains("displayname"), "{color}");
    }

    #[test]
    fn member_listing_asks_for_types_and_etags() {
        let b = propfind_members();
        assert!(b.contains("<d:resourcetype/><d:getetag/>"), "{b}");
    }
}
