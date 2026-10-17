//! CalDAV request bodies used only by the calendar service. Parsing and the shared bodies live in
//! `mcp_icloud_dav::xml`.

use chrono::{DateTime, Utc};
use mcp_icloud_dav::xml::{APPLE, CALDAV, CALSERVER};

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
}
