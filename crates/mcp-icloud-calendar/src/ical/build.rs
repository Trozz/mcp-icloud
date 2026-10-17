use crate::error::{CalError, Result};
use crate::ical::line::{Component, Property, escape_text};
use crate::ical::model::{MyAddresses, PartStat, When, normalize_address};
use crate::ical::parse::{address_of, event_from_component};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;

pub const PRODID: &str = "-//Trozz//mcp-icloud//EN";
pub const MAX_ALERT_MINUTES: i64 = 4 * 7 * 24 * 60;
const WEEKDAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "daily" => Ok(Freq::Daily),
            "weekly" => Ok(Freq::Weekly),
            "monthly" => Ok(Freq::Monthly),
            "yearly" => Ok(Freq::Yearly),
            other => Err(CalError::invalid(format!(
                "freq must be daily, weekly, monthly or yearly, not {other:?}"
            ))),
        }
    }

    pub fn as_ical(&self) -> &'static str {
        match self {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    }
}

/// The simple recurrence model accepted by the tools.
#[derive(Debug, Clone, PartialEq)]
pub struct Recurrence {
    pub freq: Freq,
    pub interval: Option<u16>,
    /// `MO`..`SU`; for monthly and yearly also with an ordinal such as `2TU` or `-1FR`.
    pub by_day: Vec<String>,
    /// 1..=31 or -31..=-1.
    pub by_month_day: Vec<i8>,
    /// Last possible occurrence (inclusive). A date means the end of that day.
    pub until: Option<When>,
    pub count: Option<u32>,
}

impl Recurrence {
    /// The RRULE value for a series starting at `start`. UNTIL is a DATE for all-day series and
    /// a UTC date-time otherwise, as RFC 5545 requires.
    pub fn to_rrule(&self, start: &When, fallback: Tz) -> Result<String> {
        let mut parts = vec![format!("FREQ={}", self.freq.as_ical())];
        match self.interval {
            Some(0) => return Err(CalError::invalid("interval must be at least 1")),
            Some(1) | None => {}
            Some(n) => parts.push(format!("INTERVAL={n}")),
        }
        if self.until.is_some() && self.count.is_some() {
            return Err(CalError::invalid("give either until or count, not both"));
        }
        if self.count == Some(0) {
            return Err(CalError::invalid("count must be at least 1"));
        }
        if !self.by_day.is_empty() {
            let days = self
                .by_day
                .iter()
                .map(|d| self.check_by_day(d))
                .collect::<Result<Vec<_>>>()?;
            parts.push(format!("BYDAY={}", days.join(",")));
        }
        if !self.by_month_day.is_empty() {
            if self.by_month_day.iter().any(|d| *d == 0 || d.abs() > 31) {
                return Err(CalError::invalid(
                    "by_month_day values must be 1 to 31 or -31 to -1",
                ));
            }
            let days: Vec<String> = self.by_month_day.iter().map(i8::to_string).collect();
            parts.push(format!("BYMONTHDAY={}", days.join(",")));
        }
        if let Some(n) = self.count {
            parts.push(format!("COUNT={n}"));
        }
        if let Some(until) = &self.until {
            let value = until_value(until, start, fallback)?;
            parts.push(format!("UNTIL={value}"));
        }
        Ok(parts.join(";"))
    }

    fn check_by_day(&self, raw: &str) -> Result<String> {
        let d = raw.trim().to_ascii_uppercase();
        let bad = || {
            CalError::invalid(format!(
                "invalid by_day value {raw:?}; use MO..SU, or for monthly and yearly an ordinal such as 2TU or -1FR"
            ))
        };
        if d.len() < 2 {
            return Err(bad());
        }
        let (ordinal, day) = d.split_at(d.len() - 2);
        if !WEEKDAYS.contains(&day) {
            return Err(bad());
        }
        if ordinal.is_empty() {
            return Ok(d);
        }
        if !matches!(self.freq, Freq::Monthly | Freq::Yearly) {
            return Err(bad());
        }
        let n: i32 = ordinal.parse().map_err(|_| bad())?;
        if n == 0 || n.abs() > 53 {
            return Err(bad());
        }
        Ok(format!("{n}{day}"))
    }
}

fn until_value(until: &When, start: &When, fallback: Tz) -> Result<String> {
    let start_instant = start.instant(fallback);
    if start.is_date() {
        let date = match until {
            When::Date(d) => *d,
            other => other
                .instant(fallback)
                .with_timezone(&fallback)
                .date_naive(),
        };
        if date < start_instant.with_timezone(&fallback).date_naive() {
            return Err(CalError::invalid("until is before the first occurrence"));
        }
        return Ok(When::Date(date).value_string());
    }
    let instant = match until {
        When::Date(d) => {
            let end_of_day = d.and_hms_opt(23, 59, 59).unwrap_or_default();
            When::Zoned {
                local: end_of_day,
                tzid: start.zone(fallback).name().to_string(),
            }
            .instant(fallback)
        }
        other => other.instant(fallback),
    };
    if instant < start_instant {
        return Err(CalError::invalid("until is before the first occurrence"));
    }
    Ok(When::Utc(instant).value_string())
}

/// Converts an inclusive all-day range into DTSTART and the exclusive DTEND.
pub fn all_day_range(start: NaiveDate, end_inclusive: NaiveDate) -> Result<(When, When)> {
    if end_inclusive < start {
        return Err(CalError::invalid("end_date is before start_date"));
    }
    Ok((
        When::Date(start),
        When::Date(end_inclusive + Duration::days(1)),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttendee {
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub summary: String,
    pub start: When,
    /// Exclusive end (DTEND).
    pub end: When,
    pub location: Option<String>,
    pub description: Option<String>,
    pub recurrence: Option<Recurrence>,
    pub alerts_minutes_before: Vec<i64>,
    pub attendees: Vec<NewAttendee>,
    /// Required when `attendees` is not empty: my address, written as ORGANIZER.
    pub organizer_email: Option<String>,
}

/// A new VCALENDAR with one VEVENT. No VTIMEZONE is written; iCloud adds it.
pub fn new_resource(
    ev: &NewEvent,
    uid: &str,
    now: DateTime<Utc>,
    fallback: Tz,
) -> Result<Component> {
    check_summary(&ev.summary)?;
    check_range(&ev.start, &ev.end, fallback)?;
    let mut vevent = Component::new("VEVENT");
    vevent.push(Property::new("UID", uid));
    vevent.push(Property::new("DTSTAMP", stamp(now)));
    vevent.push(Property::new("CREATED", stamp(now)));
    vevent.push(Property::new("LAST-MODIFIED", stamp(now)));
    vevent.push(Property::new("SEQUENCE", "0"));
    vevent.push(Property::new("SUMMARY", escape_text(ev.summary.trim())));
    vevent.push(ev.start.to_property("DTSTART"));
    vevent.push(ev.end.to_property("DTEND"));
    if let Some(l) = ev.location.as_deref().filter(|s| !s.trim().is_empty()) {
        vevent.push(Property::new("LOCATION", escape_text(l)));
    }
    if let Some(d) = ev.description.as_deref().filter(|s| !s.trim().is_empty()) {
        vevent.push(Property::new("DESCRIPTION", escape_text(d)));
    }
    if let Some(r) = &ev.recurrence {
        vevent.push(Property::new("RRULE", r.to_rrule(&ev.start, fallback)?));
    }
    vevent.push(Property::new("TRANSP", "OPAQUE"));
    set_alerts(&mut vevent, &ev.alerts_minutes_before)?;
    if !ev.attendees.is_empty() {
        let organizer = ev.organizer_email.as_deref().ok_or_else(|| {
            CalError::Internal("organizer_email is required with attendees".into())
        })?;
        let me = MyAddresses::new([organizer]);
        set_attendees(&mut vevent, &ev.attendees, organizer, &me)?;
    }
    let mut cal = Component::new("VCALENDAR");
    cal.push(Property::new("VERSION", "2.0"));
    cal.push(Property::new("PRODID", PRODID));
    cal.push(Property::new("CALSCALE", "GREGORIAN"));
    cal.components.push(vevent);
    Ok(cal)
}

/// Fields to change on an existing VEVENT; `None` leaves a field unchanged. An empty
/// `description` or `location` removes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventPatch {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: Option<When>,
    /// Exclusive end (DTEND).
    pub end: Option<When>,
    pub recurrence: Option<Recurrence>,
    pub alerts_minutes_before: Option<Vec<i64>>,
}

impl EventPatch {
    pub fn is_empty(&self) -> bool {
        *self == EventPatch::default()
    }
}

/// Applies `patch` to one VEVENT, touching only the affected properties. When only the start
/// moves, the duration is kept. SEQUENCE is bumped; use [`apply_patch_as`] for events that may
/// be organized by someone else.
pub fn apply_patch(
    vevent: &mut Component,
    patch: &EventPatch,
    now: DateTime<Utc>,
    fallback: Tz,
) -> Result<()> {
    if patch.is_empty() {
        return Err(CalError::invalid(
            "nothing to change: give at least one field",
        ));
    }
    let current = event_from_component(vevent, &MyAddresses::default())?;
    if let Some(s) = &patch.summary {
        check_summary(s)?;
        vevent.set(Property::new("SUMMARY", escape_text(s.trim())));
    }
    set_optional_text(vevent, "DESCRIPTION", patch.description.as_deref());
    set_optional_text(vevent, "LOCATION", patch.location.as_deref());
    if patch.start.is_some() || patch.end.is_some() {
        let start = patch.start.clone().unwrap_or_else(|| current.start.clone());
        let end = match (&patch.end, &patch.start) {
            (Some(e), _) => e.clone(),
            (None, Some(s)) => shifted_end(s, &current, fallback)?,
            (None, None) => current.end.clone(),
        };
        check_range(&start, &end, fallback)?;
        vevent.set(start.to_property("DTSTART"));
        vevent.remove("DURATION");
        vevent.set(end.to_property("DTEND"));
    }
    if let Some(r) = &patch.recurrence {
        if vevent.prop("RECURRENCE-ID").is_some() {
            return Err(CalError::invalid(
                "a single occurrence cannot have its own recurrence; edit the series instead",
            ));
        }
        let start = patch.start.as_ref().unwrap_or(&current.start);
        vevent.set(Property::new("RRULE", r.to_rrule(start, fallback)?));
    }
    if let Some(alerts) = &patch.alerts_minutes_before {
        set_alerts(vevent, alerts)?;
    }
    touch(vevent, now, true);
    Ok(())
}

/// Like [`apply_patch`], but SEQUENCE is bumped only when the event has no organizer or I
/// organize it (attendees must not change SEQUENCE).
pub fn apply_patch_as(
    vevent: &mut Component,
    patch: &EventPatch,
    me: &MyAddresses,
    now: DateTime<Utc>,
    fallback: Tz,
) -> Result<()> {
    let mine = organized_by(vevent, me);
    let before = vevent.value("SEQUENCE").map(str::to_string);
    apply_patch(vevent, patch, now, fallback)?;
    if !mine {
        match before {
            Some(s) => vevent.set(Property::new("SEQUENCE", s)),
            None => vevent.remove("SEQUENCE"),
        }
    }
    Ok(())
}

fn organized_by(vevent: &Component, me: &MyAddresses) -> bool {
    match vevent.prop("ORGANIZER") {
        None => true,
        Some(p) => me.matches(&address_of(p)),
    }
}

fn shifted_end(
    new_start: &When,
    current: &crate::ical::model::EventData,
    fallback: Tz,
) -> Result<When> {
    if new_start.is_date() != current.start.is_date() {
        return Err(CalError::invalid(
            "give an end as well when switching between all-day and timed",
        ));
    }
    let length = match (&current.start, &current.end) {
        (When::Date(s), When::Date(e)) => *e - *s,
        _ => current.duration(fallback),
    };
    new_start.add(length).ok_or_else(|| {
        CalError::invalid("the moved event would end outside the supported date range")
    })
}

fn set_optional_text(vevent: &mut Component, name: &str, value: Option<&str>) {
    match value {
        None => {}
        Some(v) if v.trim().is_empty() => vevent.remove(name),
        Some(v) => vevent.set(Property::new(name, escape_text(v))),
    }
}

fn check_summary(s: &str) -> Result<()> {
    if s.trim().is_empty() {
        return Err(CalError::invalid("summary must not be empty"));
    }
    Ok(())
}

fn check_range(start: &When, end: &When, fallback: Tz) -> Result<()> {
    if start.is_date() != end.is_date() {
        return Err(CalError::invalid(
            "start and end must both be dates (all-day) or both be date-times",
        ));
    }
    if end.instant(fallback) <= start.instant(fallback) {
        return Err(CalError::invalid("end must be after start"));
    }
    Ok(())
}

/// Replaces the alerting VALARMs (ACTION:NONE placeholders are kept).
fn set_alerts(vevent: &mut Component, minutes_before: &[i64]) -> Result<()> {
    if let Some(m) = minutes_before
        .iter()
        .find(|m| **m < 0 || **m > MAX_ALERT_MINUTES)
    {
        return Err(CalError::invalid(format!(
            "alert {m} is out of range; minutes before must be 0 to {MAX_ALERT_MINUTES}"
        )));
    }
    vevent.components.retain(|c| {
        !c.name.eq_ignore_ascii_case("VALARM")
            || c.value("ACTION")
                .is_some_and(|a| a.eq_ignore_ascii_case("NONE"))
    });
    for m in minutes_before {
        let mut alarm = Component::new("VALARM");
        alarm.push(Property::new("ACTION", "DISPLAY"));
        alarm.push(Property::new("DESCRIPTION", "Reminder"));
        alarm.push(Property::new("TRIGGER", format!("-PT{m}M")));
        vevent.components.push(alarm);
    }
    Ok(())
}

/// Sets the invited people. Existing ATTENDEE lines for people who stay are kept unchanged
/// (with their PARTSTAT); new people get NEEDS-ACTION and RSVP. My own line is kept. An empty
/// list turns the event back into a plain event (no ORGANIZER, no ATTENDEE).
pub fn set_attendees(
    vevent: &mut Component,
    attendees: &[NewAttendee],
    organizer_email: &str,
    me: &MyAddresses,
) -> Result<()> {
    for a in attendees {
        check_email(&a.email)?;
    }
    check_email(organizer_email)?;
    let wanted: Vec<String> = attendees
        .iter()
        .map(|a| normalize_address(&a.email))
        .collect();
    if wanted.is_empty() {
        vevent.remove("ATTENDEE");
        vevent.remove("ORGANIZER");
        return Ok(());
    }
    let mut kept: Vec<Property> = Vec::new();
    for p in vevent.props("ATTENDEE") {
        let addr = address_of(p);
        if wanted.contains(&addr) || me.matches(p.value()) || me.matches(&addr) {
            kept.push(p.clone());
        }
    }
    let has_me = kept.iter().any(|p| me.matches(&address_of(p)));
    if !has_me {
        kept.insert(
            0,
            Property::new("ATTENDEE", format!("mailto:{organizer_email}"))
                .with_param("CUTYPE", "INDIVIDUAL")
                .with_param("ROLE", "CHAIR")
                .with_param("PARTSTAT", "ACCEPTED"),
        );
    }
    for a in attendees {
        let addr = normalize_address(&a.email);
        if kept.iter().any(|p| address_of(p) == addr) {
            continue;
        }
        let mut p = Property::new("ATTENDEE", format!("mailto:{}", a.email.trim()));
        if let Some(n) = a.name.as_deref().filter(|n| !n.trim().is_empty()) {
            p.set_param("CN", n.trim());
        }
        p.set_param("CUTYPE", "INDIVIDUAL");
        p.set_param("ROLE", "REQ-PARTICIPANT");
        p.set_param("PARTSTAT", "NEEDS-ACTION");
        p.set_param("RSVP", "TRUE");
        kept.push(p);
    }
    vevent.remove("ATTENDEE");
    if vevent.prop("ORGANIZER").is_none() {
        vevent.push(Property::new(
            "ORGANIZER",
            format!("mailto:{organizer_email}"),
        ));
    }
    for p in kept {
        vevent.push(p);
    }
    Ok(())
}

/// Sets my PARTSTAT on an invitation I received. SEQUENCE is left alone.
pub fn set_my_partstat(
    vevent: &mut Component,
    me: &MyAddresses,
    status: &PartStat,
    now: DateTime<Utc>,
) -> Result<()> {
    let mine = vevent.properties.iter_mut().find(|p| {
        p.name() == "ATTENDEE"
            && (me.matches(p.value()) || p.param("EMAIL").is_some_and(|e| me.matches(e)))
    });
    let Some(p) = mine else {
        return Err(CalError::invalid("you are not an attendee of this event"));
    };
    p.set_param("PARTSTAT", status.as_ical());
    p.remove_param("RSVP");
    touch(vevent, now, false);
    Ok(())
}

fn check_email(email: &str) -> Result<()> {
    let e = email.trim();
    let valid = e.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
    }) && !e.contains(|c: char| c.is_whitespace() || "<>:;,\"".contains(c));
    if !valid {
        return Err(CalError::invalid(format!(
            "invalid email address {email:?}"
        )));
    }
    Ok(())
}

/// Updates DTSTAMP and LAST-MODIFIED, and bumps SEQUENCE when asked.
pub fn touch(vevent: &mut Component, now: DateTime<Utc>, bump_sequence: bool) {
    vevent.set(Property::new("DTSTAMP", stamp(now)));
    vevent.set(Property::new("LAST-MODIFIED", stamp(now)));
    if bump_sequence {
        let seq: u32 = vevent
            .value("SEQUENCE")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        vevent.set(Property::new("SEQUENCE", (seq + 1).to_string()));
    }
}

pub fn stamp(now: DateTime<Utc>) -> String {
    When::Utc(now).value_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ical::line::unfold;
    use crate::ical::model::When;
    use crate::ical::parse::{events, master, parse_resource};
    use chrono::NaiveDateTime;
    use chrono_tz::Europe;

    const APPLE: &str = include_str!("../../tests/fixtures/apple-allday.ics");
    const GOOGLE: &str = include_str!("../../tests/fixtures/google-invite.ics");

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn zoned(s: &str) -> When {
        When::Zoned {
            local: NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap(),
            tzid: "Europe/London".into(),
        }
    }

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn basic(start: When, end: When) -> NewEvent {
        NewEvent {
            summary: "Dentist".into(),
            start,
            end,
            location: None,
            description: None,
            recurrence: None,
            alerts_minutes_before: vec![],
            attendees: vec![],
            organizer_email: None,
        }
    }

    fn weekly() -> Recurrence {
        Recurrence {
            freq: Freq::Weekly,
            interval: None,
            by_day: vec![],
            by_month_day: vec![],
            until: None,
            count: None,
        }
    }

    fn me() -> MyAddresses {
        MyAddresses::new(["mailto:user@example.com"])
    }

    #[test]
    fn new_timed_event_has_tzid_and_no_vtimezone() {
        let mut ev = basic(zoned("2026-11-03T09:00"), zoned("2026-11-03T09:30"));
        ev.location = Some("Example Clinic, Room 2".into());
        ev.alerts_minutes_before = vec![30];
        let cal = new_resource(&ev, "uid-1@example.com", now(), Europe::London).unwrap();
        let ics = cal.to_ics();
        assert!(ics.contains("PRODID:-//Trozz//mcp-icloud//EN\r\n"));
        assert!(ics.contains("DTSTART;TZID=Europe/London:20261103T090000\r\n"));
        assert!(ics.contains("DTEND;TZID=Europe/London:20261103T093000\r\n"));
        assert!(ics.contains("LOCATION:Example Clinic\\, Room 2\r\n"));
        assert!(ics.contains("TRIGGER:-PT30M\r\n"));
        assert!(ics.contains("DTSTAMP:20261017T120000Z\r\n"));
        assert!(!ics.contains("VTIMEZONE"));
        let back = parse_resource(&ics).unwrap();
        let e = &events(&back, &me()).unwrap()[0];
        assert_eq!(e.uid, "uid-1@example.com");
        assert_eq!(e.location.as_deref(), Some("Example Clinic, Room 2"));
        assert_eq!(e.alerts_minutes_before, [30]);
    }

    #[test]
    fn inclusive_all_day_end_becomes_exclusive_dtend() {
        let (start, end) = all_day_range(date("2026-12-24"), date("2026-12-26")).unwrap();
        let cal = new_resource(&basic(start, end), "u", now(), Europe::London).unwrap();
        let ics = cal.to_ics();
        assert!(ics.contains("DTSTART;VALUE=DATE:20261224\r\n"));
        assert!(ics.contains("DTEND;VALUE=DATE:20261227\r\n"));
        let one_day = all_day_range(date("2026-12-25"), date("2026-12-25")).unwrap();
        assert_eq!(one_day.1, When::Date(date("2026-12-26")));
        assert_eq!(
            all_day_range(date("2026-12-26"), date("2026-12-24"))
                .unwrap_err()
                .kind(),
            "validation"
        );
    }

    #[test]
    fn bad_ranges_and_summaries_are_rejected() {
        let e = new_resource(
            &basic(zoned("2026-11-03T09:00"), zoned("2026-11-03T09:00")),
            "u",
            now(),
            Europe::London,
        )
        .unwrap_err();
        assert!(e.to_string().contains("end must be after start"));
        let e = new_resource(
            &basic(When::Date(date("2026-11-03")), zoned("2026-11-04T09:00")),
            "u",
            now(),
            Europe::London,
        )
        .unwrap_err();
        assert_eq!(e.kind(), "validation");
        let mut ev = basic(zoned("2026-11-03T09:00"), zoned("2026-11-03T10:00"));
        ev.summary = "  ".into();
        assert_eq!(
            new_resource(&ev, "u", now(), Europe::London)
                .unwrap_err()
                .kind(),
            "validation"
        );
        ev.summary = "x".into();
        ev.alerts_minutes_before = vec![-5];
        assert_eq!(
            new_resource(&ev, "u", now(), Europe::London)
                .unwrap_err()
                .kind(),
            "validation"
        );
    }

    #[test]
    fn recurrence_rules() {
        let start = zoned("2026-11-03T09:00");
        let mut r = weekly();
        r.by_day = vec!["mo".into(), "WE".into()];
        r.interval = Some(2);
        r.count = Some(10);
        assert_eq!(
            r.to_rrule(&start, Europe::London).unwrap(),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10"
        );
        let monthly = Recurrence {
            freq: Freq::Monthly,
            by_day: vec!["2TU".into(), "-1fr".into()],
            ..weekly()
        };
        assert_eq!(
            monthly.to_rrule(&start, Europe::London).unwrap(),
            "FREQ=MONTHLY;BYDAY=2TU,-1FR"
        );
        let by_month_day = Recurrence {
            freq: Freq::Monthly,
            by_month_day: vec![1, -1],
            ..weekly()
        };
        assert_eq!(
            by_month_day.to_rrule(&start, Europe::London).unwrap(),
            "FREQ=MONTHLY;BYMONTHDAY=1,-1"
        );
    }

    #[test]
    fn until_is_utc_for_timed_and_a_date_for_all_day() {
        let mut r = weekly();
        r.until = Some(When::Date(date("2026-12-31")));
        assert_eq!(
            r.to_rrule(&zoned("2026-11-03T09:00"), Europe::London)
                .unwrap(),
            "FREQ=WEEKLY;UNTIL=20261231T235959Z"
        );
        assert_eq!(
            r.to_rrule(&When::Date(date("2026-11-03")), Europe::London)
                .unwrap(),
            "FREQ=WEEKLY;UNTIL=20261231"
        );
        let summer = Recurrence {
            until: Some(When::Date(date("2027-07-01"))),
            ..weekly()
        };
        assert_eq!(
            summer
                .to_rrule(&zoned("2026-11-03T09:00"), Europe::London)
                .unwrap(),
            "FREQ=WEEKLY;UNTIL=20270701T225959Z"
        );
    }

    #[test]
    fn invalid_recurrences_are_rejected() {
        let start = zoned("2026-11-03T09:00");
        let cases = [
            Recurrence {
                count: Some(3),
                until: Some(When::Date(date("2026-12-01"))),
                ..weekly()
            },
            Recurrence {
                count: Some(0),
                ..weekly()
            },
            Recurrence {
                interval: Some(0),
                ..weekly()
            },
            Recurrence {
                by_day: vec!["2TU".into()],
                ..weekly()
            },
            Recurrence {
                by_day: vec!["XX".into()],
                ..weekly()
            },
            Recurrence {
                freq: Freq::Monthly,
                by_day: vec!["0MO".into()],
                ..weekly()
            },
            Recurrence {
                freq: Freq::Monthly,
                by_month_day: vec![32],
                ..weekly()
            },
            Recurrence {
                until: Some(When::Date(date("2026-01-01"))),
                ..weekly()
            },
        ];
        for r in cases {
            let e = r.to_rrule(&start, Europe::London).unwrap_err();
            assert_eq!(e.kind(), "validation", "{r:?}");
        }
        assert_eq!(Freq::parse("Weekly").unwrap(), Freq::Weekly);
        assert_eq!(Freq::parse("hourly").unwrap_err().kind(), "validation");
    }

    #[test]
    fn patch_touches_only_the_affected_lines() {
        let mut cal = parse_resource(APPLE).unwrap();
        let ev = cal.children_mut("VEVENT").next().unwrap();
        let patch = EventPatch {
            summary: Some("Hotel stay".into()),
            ..Default::default()
        };
        apply_patch(ev, &patch, now(), Europe::London).unwrap();
        let before = unfold(APPLE);
        let after = unfold(&cal.to_ics());
        let changed: Vec<&String> = after.iter().filter(|l| !before.contains(l)).collect();
        assert_eq!(
            changed,
            [
                "DTSTAMP:20261017T120000Z",
                "LAST-MODIFIED:20261017T120000Z",
                "SEQUENCE:1",
                "SUMMARY:Hotel stay"
            ]
        );
        let key = before
            .iter()
            .find(|l| l.starts_with("X-APPLE-SUGGESTION-INFO-UNIQUE-KEY"))
            .unwrap();
        assert!(after.contains(key));
        assert_eq!(before.len(), after.len());
    }

    #[test]
    fn moving_an_all_day_start_keeps_whole_days_across_a_dst_change() {
        for (start, end, days) in [
            ("2027-03-28", "2027-03-29", 1),
            ("2027-03-27", "2027-03-30", 3),
        ] {
            let ev = basic(When::Date(date(start)), When::Date(date(end)));
            let mut cal = new_resource(&ev, "u", now(), Europe::London).unwrap();
            let vevent = cal.children_mut("VEVENT").next().unwrap();
            let patch = EventPatch {
                start: Some(When::Date(date("2027-04-10"))),
                ..Default::default()
            };
            apply_patch(vevent, &patch, now(), Europe::London).unwrap();
            let ics = cal.to_ics();
            let end = date("2027-04-10") + Duration::days(days);
            assert!(ics.contains("DTSTART;VALUE=DATE:20270410\r\n"));
            assert!(ics.contains(&format!("DTEND;VALUE=DATE:{}\r\n", end.format("%Y%m%d"))));
        }
    }

    #[test]
    fn moving_the_start_keeps_the_duration() {
        let ev = basic(zoned("2026-11-03T09:00"), zoned("2026-11-03T09:45"));
        let mut cal = new_resource(&ev, "u", now(), Europe::London).unwrap();
        let vevent = cal.children_mut("VEVENT").next().unwrap();
        let patch = EventPatch {
            start: Some(zoned("2026-11-04T14:00")),
            ..Default::default()
        };
        apply_patch(vevent, &patch, now(), Europe::London).unwrap();
        let ics = cal.to_ics();
        assert!(ics.contains("DTSTART;TZID=Europe/London:20261104T140000\r\n"));
        assert!(ics.contains("DTEND;TZID=Europe/London:20261104T144500\r\n"));
        let vevent = cal.children_mut("VEVENT").next().unwrap();
        let to_all_day = EventPatch {
            start: Some(When::Date(date("2026-11-05"))),
            ..Default::default()
        };
        assert_eq!(
            apply_patch(vevent, &to_all_day, now(), Europe::London)
                .unwrap_err()
                .kind(),
            "validation"
        );
    }

    #[test]
    fn empty_text_removes_and_empty_patch_is_rejected() {
        let mut cal = parse_resource(APPLE).unwrap();
        let ev = cal.children_mut("VEVENT").next().unwrap();
        let patch = EventPatch {
            location: Some(String::new()),
            alerts_minutes_before: Some(vec![]),
            ..Default::default()
        };
        apply_patch(ev, &patch, now(), Europe::London).unwrap();
        assert!(ev.prop("LOCATION").is_none());
        assert!(ev.children("VALARM").next().is_none());
        assert_eq!(
            apply_patch(ev, &EventPatch::default(), now(), Europe::London)
                .unwrap_err()
                .kind(),
            "validation"
        );
    }

    #[test]
    fn attendee_edits_on_an_invitation_keep_sequence() {
        let mut cal = parse_resource(GOOGLE).unwrap();
        let ev = cal.children_mut("VEVENT").next().unwrap();
        let patch = EventPatch {
            alerts_minutes_before: Some(vec![60]),
            ..Default::default()
        };
        apply_patch_as(ev, &patch, &me(), now(), Europe::London).unwrap();
        assert_eq!(ev.value("SEQUENCE"), Some("0"));
        assert_eq!(ev.props("ATTENDEE").count(), 2);
        let mut own = basic(zoned("2026-11-03T09:00"), zoned("2026-11-03T10:00"));
        own.attendees = vec![NewAttendee {
            email: "friend@example.org".into(),
            name: None,
        }];
        own.organizer_email = Some("user@example.com".into());
        let mut cal = new_resource(&own, "u", now(), Europe::London).unwrap();
        let ev = master_mut_of(&mut cal);
        apply_patch_as(ev, &patch, &me(), now(), Europe::London).unwrap();
        assert_eq!(ev.value("SEQUENCE"), Some("1"));
    }

    #[test]
    fn replying_sets_only_my_partstat() {
        let mut cal = parse_resource(GOOGLE).unwrap();
        let ev = cal.children_mut("VEVENT").next().unwrap();
        set_my_partstat(ev, &me(), &PartStat::Accepted, now()).unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        assert_eq!(e.my_status(), Some(&PartStat::Accepted));
        let organiser = e.attendees.iter().find(|a| !a.is_me).unwrap();
        assert_eq!(organiser.status, PartStat::Accepted);
        assert_eq!(e.sequence, 0);
        let mut plain = parse_resource(APPLE).unwrap();
        let ev = plain.children_mut("VEVENT").next().unwrap();
        assert_eq!(
            set_my_partstat(ev, &me(), &PartStat::Declined, now())
                .unwrap_err()
                .kind(),
            "validation"
        );
    }

    #[test]
    fn attendees_are_added_kept_and_removed() {
        let mut ev = basic(zoned("2026-11-03T09:00"), zoned("2026-11-03T10:00"));
        ev.attendees = vec![NewAttendee {
            email: "friend@example.org".into(),
            name: Some("Friend".into()),
        }];
        ev.organizer_email = Some("user@example.com".into());
        let mut cal = new_resource(&ev, "u", now(), Europe::London).unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        assert!(e.i_organize());
        assert_eq!(e.attendees.len(), 2);
        let friend = e.attendees.iter().find(|a| !a.is_me).unwrap();
        assert_eq!(friend.status, PartStat::NeedsAction);
        assert_eq!(friend.name.as_deref(), Some("Friend"));

        let vevent = master_mut_of(&mut cal);
        let mut friend_line = vevent
            .props("ATTENDEE")
            .find(|p| p.value() == "mailto:friend@example.org")
            .unwrap()
            .clone();
        friend_line.set_param("PARTSTAT", "ACCEPTED");
        vevent.set(friend_line);
        let both = [
            NewAttendee {
                email: "Friend@Example.org".into(),
                name: None,
            },
            NewAttendee {
                email: "other@example.net".into(),
                name: None,
            },
        ];
        set_attendees(vevent, &both, "user@example.com", &me()).unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        let friend = e
            .attendees
            .iter()
            .find(|a| a.email == "friend@example.org")
            .unwrap();
        assert_eq!(friend.status, PartStat::Accepted);
        assert!(e.attendees.iter().any(|a| a.email == "other@example.net"));

        set_attendees(master_mut_of(&mut cal), &[], "user@example.com", &me()).unwrap();
        let e = &events(&cal, &me()).unwrap()[0];
        assert!(e.attendees.is_empty());
        assert!(e.organizer.is_none());
        assert_eq!(
            set_attendees(
                master_mut_of(&mut cal),
                &[NewAttendee {
                    email: "not an email".into(),
                    name: None
                }],
                "user@example.com",
                &me()
            )
            .unwrap_err()
            .kind(),
            "validation"
        );
        assert!(master(&cal).is_some());
    }

    fn master_mut_of(cal: &mut Component) -> &mut Component {
        crate::ical::parse::master_mut(cal).unwrap()
    }
}
