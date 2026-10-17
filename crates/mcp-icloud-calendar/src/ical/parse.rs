use crate::error::{CalError, Result};
use crate::ical::line::{Component, Property, unescape_text};
use crate::ical::model::{
    Attendee, EventData, MyAddresses, Organizer, PartStat, When, normalize_address, when_list,
};
use chrono::Duration;

/// Parses a CalDAV resource and checks that it is a VCALENDAR.
pub fn parse_resource(text: &str) -> Result<Component> {
    let cal = Component::parse(text)?;
    if !cal.name.eq_ignore_ascii_case("VCALENDAR") {
        return Err(CalError::Upstream(format!(
            "expected VCALENDAR, found {}",
            cal.name
        )));
    }
    Ok(cal)
}

/// The series master (or the single event): the VEVENT without RECURRENCE-ID.
pub fn master(cal: &Component) -> Option<&Component> {
    cal.children("VEVENT")
        .find(|c| c.prop("RECURRENCE-ID").is_none())
}

pub fn master_mut(cal: &mut Component) -> Option<&mut Component> {
    cal.children_mut("VEVENT")
        .find(|c| c.prop("RECURRENCE-ID").is_none())
}

/// VEVENTs that carry a RECURRENCE-ID.
pub fn overrides(cal: &Component) -> impl Iterator<Item = &Component> {
    cal.children("VEVENT")
        .filter(|c| c.prop("RECURRENCE-ID").is_some())
}

/// Every VEVENT in the resource, master first when present.
pub fn events(cal: &Component, me: &MyAddresses) -> Result<Vec<EventData>> {
    let mut out = Vec::new();
    if let Some(m) = master(cal) {
        out.push(event_from_component(m, me)?);
    }
    for o in overrides(cal) {
        out.push(event_from_component(o, me)?);
    }
    Ok(out)
}

pub fn event_from_component(c: &Component, me: &MyAddresses) -> Result<EventData> {
    let start_prop = c
        .prop("DTSTART")
        .ok_or_else(|| CalError::Upstream("VEVENT without DTSTART".into()))?;
    let start = When::from_property(start_prop)?;
    let end = match (c.prop("DTEND"), c.value("DURATION")) {
        (Some(p), _) => When::from_property(p)?,
        (None, Some(d)) => start.add(
            parse_duration(d)
                .ok_or_else(|| CalError::Upstream(format!("invalid DURATION value {d:?}")))?,
        ),
        (None, None) if start.is_date() => start.add(Duration::days(1)),
        (None, None) => start.clone(),
    };
    Ok(EventData {
        uid: c.value("UID").unwrap_or_default().to_string(),
        recurrence_id: c
            .prop("RECURRENCE-ID")
            .map(When::from_property)
            .transpose()?,
        summary: text(c, "SUMMARY"),
        description: text(c, "DESCRIPTION"),
        location: text(c, "LOCATION"),
        url: c.value("URL").map(str::to_string).filter(|s| !s.is_empty()),
        start,
        end,
        rrule: c.value("RRULE").map(str::to_string),
        exdates: all_whens(c, "EXDATE")?,
        rdates: all_whens(c, "RDATE")?,
        attendees: c.props("ATTENDEE").map(|p| attendee(p, me)).collect(),
        organizer: c.prop("ORGANIZER").map(|p| organizer(p, me)),
        status: c.value("STATUS").map(|s| s.to_ascii_uppercase()),
        transparent: c
            .value("TRANSP")
            .is_some_and(|t| t.eq_ignore_ascii_case("TRANSPARENT")),
        alerts_minutes_before: alerts(c),
        sequence: c
            .value("SEQUENCE")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0),
    })
}

fn text(c: &Component, name: &str) -> Option<String> {
    c.value(name)
        .map(unescape_text)
        .filter(|s| !s.trim().is_empty())
}

fn all_whens(c: &Component, name: &str) -> Result<Vec<When>> {
    let mut out = Vec::new();
    for p in c.props(name) {
        out.extend(when_list(p)?);
    }
    Ok(out)
}

/// The address an ATTENDEE or ORGANIZER stands for: its mailto value, else its EMAIL parameter.
pub fn address_of(p: &Property) -> String {
    let value = p.value().trim();
    if value.len() >= 7 && value[..7].eq_ignore_ascii_case("mailto:") {
        return normalize_address(value);
    }
    match p.param("EMAIL") {
        Some(e) => normalize_address(e),
        None => normalize_address(value),
    }
}

fn is_me(p: &Property, me: &MyAddresses) -> bool {
    me.matches(p.value()) || p.param("EMAIL").is_some_and(|e| me.matches(e))
}

fn attendee(p: &Property, me: &MyAddresses) -> Attendee {
    Attendee {
        email: address_of(p),
        name: p.param("CN").map(str::to_string),
        status: PartStat::parse(p.param("PARTSTAT").unwrap_or("NEEDS-ACTION")),
        role: p.param("ROLE").map(str::to_string),
        is_me: is_me(p, me),
    }
}

fn organizer(p: &Property, me: &MyAddresses) -> Organizer {
    Organizer {
        email: address_of(p),
        name: p.param("CN").map(str::to_string),
        is_me: is_me(p, me),
    }
}

/// Minutes before the start for each relative, start-related VALARM that alerts the user.
fn alerts(c: &Component) -> Vec<i64> {
    let mut out = Vec::new();
    for alarm in c.children("VALARM") {
        if alarm
            .value("ACTION")
            .is_some_and(|a| a.eq_ignore_ascii_case("NONE"))
        {
            continue;
        }
        let Some(trigger) = alarm.prop("TRIGGER") else {
            continue;
        };
        if trigger
            .param("VALUE")
            .is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"))
            || trigger
                .param("RELATED")
                .is_some_and(|r| r.eq_ignore_ascii_case("END"))
        {
            continue;
        }
        if let Some(d) = parse_duration(trigger.value()) {
            out.push(-d.num_minutes());
        }
    }
    out
}

/// RFC 5545 DURATION: `[+-]P[nW][nD][T[nH][nM][nS]]`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (sign, rest) = match s.as_bytes().first()? {
        b'-' => (-1, &s[1..]),
        b'+' => (1, &s[1..]),
        _ => (1, s),
    };
    let rest = rest.strip_prefix(['P', 'p'])?;
    let mut total = Duration::zero();
    let mut in_time = false;
    let mut number = String::new();
    let mut seen = false;
    for ch in rest.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        let ch = ch.to_ascii_uppercase();
        if ch == 'T' {
            in_time = true;
            continue;
        }
        let n: i64 = number.parse().ok()?;
        number.clear();
        seen = true;
        total += match (ch, in_time) {
            ('W', false) => Duration::weeks(n),
            ('D', false) => Duration::days(n),
            ('H', true) => Duration::hours(n),
            ('M', true) => Duration::minutes(n),
            ('S', true) => Duration::seconds(n),
            _ => return None,
        };
    }
    if !number.is_empty() || !seen {
        return None;
    }
    Some(total * sign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::{Europe, Tz};

    const APPLE: &str = include_str!("../../tests/fixtures/apple-allday.ics");
    const GOOGLE: &str = include_str!("../../tests/fixtures/google-invite.ics");
    const OUTLOOK: &str = include_str!("../../tests/fixtures/outlook-invite.ics");
    const OVERRIDE: &str = include_str!("../../tests/fixtures/master-override.ics");

    fn me() -> MyAddresses {
        MyAddresses::new([
            "mailto:user@example.com",
            "/1234567890/principal/",
            "urn:uuid:1234567890",
        ])
    }

    #[test]
    fn apple_all_day_event() {
        let cal = parse_resource(APPLE).unwrap();
        let evs = events(&cal, &me()).unwrap();
        assert_eq!(evs.len(), 1);
        let e = &evs[0];
        assert_eq!(e.uid, "1C9196A1-4B2B-4BB0-B400-0DA9A6250616");
        assert_eq!(e.summary.as_deref(), Some("Stay at Example Hotel"));
        assert_eq!(
            e.description.as_deref(),
            Some("Check-in from 3pm\nBring the booking reference, please")
        );
        assert_eq!(e.location.as_deref(), Some("Example Hotel, 1 High Street"));
        assert!(e.start.is_date());
        assert_eq!(e.end.value_string(), "20260118");
        assert_eq!(e.alerts_minutes_before, [900]);
        assert!(!e.transparent);
        assert!(e.url.as_deref().unwrap().ends_with("booking-1@example.com"));
        assert!(!e.is_recurring());
    }

    #[test]
    fn google_invite_attendees_and_me() {
        let cal = parse_resource(GOOGLE).unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        assert_eq!(e.attendees.len(), 2);
        let mine = e.attendees.iter().find(|a| a.is_me).unwrap();
        assert_eq!(mine.email, "user@example.com");
        assert_eq!(mine.name.as_deref(), Some("User, Example"));
        assert_eq!(e.my_status(), Some(&PartStat::NeedsAction));
        assert!(!e.i_organize());
        assert!(e.has_other_attendees());
        assert_eq!(e.organizer.as_ref().unwrap().email, "organiser@example.org");
        assert_eq!(e.alerts_minutes_before, [10]);
        assert_eq!(e.location, None);
        assert_eq!(e.status.as_deref(), Some("CONFIRMED"));
    }

    #[test]
    fn outlook_invite_matches_me_by_email_param() {
        let cal = parse_resource(OUTLOOK).unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        assert_eq!(e.my_status(), Some(&PartStat::Tentative));
        let mine = e.attendees.iter().find(|a| a.is_me).unwrap();
        assert_eq!(mine.email, "user@example.com");
        assert_eq!(e.summary.as_deref(), Some("Budget sync"));
        assert_eq!(e.sequence, 2);
        assert_eq!(e.start.tzid(), Some("GMT Standard Time"));
        assert_eq!(e.duration(Europe::London), Duration::minutes(30));
    }

    #[test]
    fn master_and_overrides_are_separated() {
        let cal = parse_resource(OVERRIDE).unwrap();
        let evs = events(&cal, &me()).unwrap();
        assert_eq!(evs.len(), 3);
        assert!(evs[0].recurrence_id.is_none());
        assert_eq!(evs[0].rrule.as_deref(), Some("FREQ=WEEKLY;COUNT=6"));
        assert_eq!(evs[0].exdates.len(), 1);
        assert!(evs[0].is_recurring());
        assert_eq!(evs[1].summary.as_deref(), Some("Planning (moved)"));
        assert!(evs[2].is_cancelled());
        assert_eq!(overrides(&cal).count(), 2);
        assert!(master(&cal).is_some());
    }

    #[test]
    fn missing_end_uses_duration_or_defaults() {
        let base = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\n";
        let cal = parse_resource(&format!(
            "{base}DTSTART:20260101T090000Z\nDURATION:PT1H30M\nEND:VEVENT\nEND:VCALENDAR\n"
        ))
        .unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        assert_eq!(e.duration(Tz::UTC), Duration::minutes(90));
        let cal = parse_resource(&format!(
            "{base}DTSTART;VALUE=DATE:20260101\nEND:VEVENT\nEND:VCALENDAR\n"
        ))
        .unwrap();
        assert_eq!(
            events(&cal, &me()).unwrap()[0].end.value_string(),
            "20260102"
        );
        let cal = parse_resource(&format!(
            "{base}DTSTART:20260101T090000Z\nEND:VEVENT\nEND:VCALENDAR\n"
        ))
        .unwrap();
        assert_eq!(
            events(&cal, &me()).unwrap()[0].duration(Tz::UTC),
            Duration::zero()
        );
    }

    #[test]
    fn event_without_dtstart_is_an_upstream_error() {
        let cal =
            parse_resource("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nEND:VEVENT\nEND:VCALENDAR\n")
                .unwrap();
        assert_eq!(events(&cal, &me()).unwrap_err().kind(), "upstream");
        assert_eq!(
            parse_resource("BEGIN:VEVENT\nEND:VEVENT\n")
                .unwrap_err()
                .kind(),
            "upstream"
        );
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(parse_duration("-P0DT0H10M0S"), Some(Duration::minutes(-10)));
        assert_eq!(parse_duration("P1W"), Some(Duration::weeks(1)));
        assert_eq!(parse_duration("+P1DT2H"), Some(Duration::hours(26)));
        assert_eq!(parse_duration("PT"), None);
        assert_eq!(parse_duration("P1H"), None);
        assert_eq!(parse_duration("15M"), None);
    }

    #[test]
    fn alarms_that_do_not_alert_are_skipped() {
        let cal = parse_resource(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:20260101T090000Z\n\
             BEGIN:VALARM\nACTION:NONE\nTRIGGER;VALUE=DATE-TIME:19760401T005545Z\nEND:VALARM\n\
             BEGIN:VALARM\nACTION:DISPLAY\nTRIGGER;RELATED=END:-PT5M\nEND:VALARM\n\
             BEGIN:VALARM\nACTION:AUDIO\nTRIGGER:-PT1H\nEND:VALARM\n\
             END:VEVENT\nEND:VCALENDAR\n",
        )
        .unwrap();
        assert_eq!(events(&cal, &me()).unwrap()[0].alerts_minutes_before, [60]);
    }
}
