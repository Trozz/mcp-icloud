//! JSON views of events for tool output, and conversion of tool input into iCalendar values.

use crate::error::Result;
use crate::ical::model::{EventData, When};
use crate::tz::{format_in, parse_date, parse_datetime, resolve_local};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

/// `YYYY-MM-DD` for dates, RFC 3339 in `zone` for times.
pub fn when_out(w: &When, zone: Tz) -> String {
    match w {
        When::Date(d) => d.format("%Y-%m-%d").to_string(),
        other => format_in(other.instant(zone), zone),
    }
}

/// Start and end fields: `all_day` plus `start_date`/`end_date` (inclusive) for all-day
/// values, or `start`/`end` in `zone`. `event_timezone` is added when the event is stored in
/// another zone.
pub fn span_fields(start: &When, end: &When, zone: Tz) -> Map<String, Value> {
    let mut m = Map::new();
    if let (When::Date(s), When::Date(e)) = (start, end) {
        let last = if e > s { *e - Duration::days(1) } else { *s };
        m.insert("all_day".into(), json!(true));
        m.insert("start_date".into(), json!(s.format("%Y-%m-%d").to_string()));
        m.insert(
            "end_date".into(),
            json!(last.format("%Y-%m-%d").to_string()),
        );
        return m;
    }
    m.insert("all_day".into(), json!(false));
    m.insert("start".into(), json!(when_out(start, zone)));
    m.insert("end".into(), json!(when_out(end, zone)));
    if let Some(tzid) = start.tzid()
        && tzid != zone.name()
    {
        m.insert("event_timezone".into(), json!(tzid));
    }
    m
}

/// The value a client passes back as `occurrence_start`.
pub fn occurrence_out(rid: &When, zone: Tz) -> String {
    when_out(rid, zone)
}

/// `occurrence_start` input: a date for all-day series, else a date-time.
pub fn occurrence_in(input: &str, zone: Tz) -> Result<When> {
    let s = input.trim();
    if s.len() == 10 {
        return Ok(When::Date(parse_date(s)?));
    }
    Ok(When::Utc(parse_datetime(s, zone)?))
}

/// A timed tool input stored with the zone's TZID (iCloud adds the VTIMEZONE).
pub fn zoned(input: &str, zone: Tz) -> Result<When> {
    let instant = parse_datetime(input, zone)?;
    Ok(When::Zoned {
        local: instant.with_timezone(&zone).naive_local(),
        tzid: zone.name().to_string(),
    })
}

/// A range bound: a date means midnight in `zone`, otherwise a date-time.
pub fn bound(input: &str, zone: Tz) -> Result<DateTime<Utc>> {
    let s = input.trim();
    if s.len() == 10 {
        let d = parse_date(s)?;
        return resolve_local(d.and_time(NaiveTime::MIN), zone);
    }
    parse_datetime(s, zone)
}

/// An `until` value: a date includes that whole day; all-day series use a DATE.
pub fn until_in(input: &str, all_day: bool, zone: Tz) -> Result<When> {
    let s = input.trim();
    if s.len() == 10 {
        let d = parse_date(s)?;
        if all_day {
            return Ok(When::Date(d));
        }
        let end_of_day = d.and_hms_opt(23, 59, 59).unwrap_or_default();
        return Ok(When::Utc(resolve_local(end_of_day, zone)?));
    }
    let t = parse_datetime(s, zone)?;
    if all_day {
        return Ok(When::Date(t.with_timezone(&zone).date_naive()));
    }
    Ok(When::Utc(t))
}

const DAY_NAMES: [(&str, &str); 7] = [
    ("MO", "Monday"),
    ("TU", "Tuesday"),
    ("WE", "Wednesday"),
    ("TH", "Thursday"),
    ("FR", "Friday"),
    ("SA", "Saturday"),
    ("SU", "Sunday"),
];

fn ordinal(n: i32) -> String {
    match n {
        -1 => "last".into(),
        1 => "1st".into(),
        2 => "2nd".into(),
        3 => "3rd".into(),
        n if n < 0 => format!("{}th from last", -n),
        n => format!("{n}th"),
    }
}

fn day_words(spec: &str) -> String {
    let code = &spec[spec.len().saturating_sub(2)..];
    let name = DAY_NAMES
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(code))
        .map(|(_, n)| *n)
        .unwrap_or(code);
    let prefix = &spec[..spec.len().saturating_sub(2)];
    match prefix.parse::<i32>() {
        Ok(n) => format!("{} {name}", ordinal(n)),
        Err(_) => name.to_string(),
    }
}

/// Plain words for an RRULE, e.g. "every 2 weeks on Monday, Wednesday, 10 times".
pub fn describe_rrule(rrule: &str) -> String {
    let mut freq = "";
    let mut interval = 1u32;
    let mut parts = Vec::new();
    let mut tail = Vec::new();
    for kv in rrule.split(';') {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => freq = v,
            "INTERVAL" => interval = v.parse().unwrap_or(1),
            "BYDAY" => parts.push(format!(
                "on {}",
                v.split(',').map(day_words).collect::<Vec<_>>().join(", ")
            )),
            "BYMONTHDAY" => parts.push(format!(
                "on day {}",
                v.split(',').collect::<Vec<_>>().join(", ")
            )),
            "COUNT" => tail.push(format!("{v} times")),
            "UNTIL" => {
                let date = v.get(..8).unwrap_or(v);
                let pretty = NaiveDate::parse_from_str(date, "%Y%m%d")
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|_| v.to_string());
                tail.push(format!("until {pretty}"));
            }
            _ => {}
        }
    }
    let unit = match freq.to_ascii_uppercase().as_str() {
        "DAILY" => "day",
        "WEEKLY" => "week",
        "MONTHLY" => "month",
        "YEARLY" => "year",
        "HOURLY" => "hour",
        "MINUTELY" => "minute",
        _ => return format!("custom rule {rrule}"),
    };
    let mut text = if interval > 1 {
        format!("every {interval} {unit}s")
    } else {
        format!("every {unit}")
    };
    for p in parts {
        text.push(' ');
        text.push_str(&p);
    }
    for t in tail {
        text.push_str(", ");
        text.push_str(&t);
    }
    text
}

/// Fields shared by list and detail views of one occurrence or event.
pub fn event_fields(ev: &EventData, start: &When, end: &When, zone: Tz) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("summary".into(), json!(ev.summary));
    m.extend(span_fields(start, end, zone));
    if let Some(l) = &ev.location {
        m.insert("location".into(), json!(l));
    }
    m.insert(
        "recurring".into(),
        json!(ev.is_recurring() || ev.recurrence_id.is_some()),
    );
    if let Some(s) = ev.my_status() {
        m.insert("my_status".into(), json!(s.as_output()));
    }
    if ev.is_cancelled() {
        m.insert("cancelled".into(), json!(true));
    }
    m
}

/// Full detail for `get_event`.
pub fn event_detail(ev: &EventData, start: &When, end: &When, zone: Tz) -> Map<String, Value> {
    let mut m = event_fields(ev, start, end, zone);
    m.insert("description".into(), json!(ev.description));
    m.insert("url".into(), json!(ev.url));
    m.insert("busy".into(), json!(!ev.transparent));
    m.insert("status".into(), json!(ev.status));
    m.insert(
        "alerts_minutes_before".into(),
        json!(ev.alerts_minutes_before),
    );
    if let Some(rule) = &ev.rrule {
        m.insert(
            "recurrence".into(),
            json!({ "text": describe_rrule(rule), "rrule": rule }),
        );
    }
    if !ev.exdates.is_empty() {
        let ex: Vec<String> = ev.exdates.iter().map(|w| when_out(w, zone)).collect();
        m.insert("excluded_occurrences".into(), json!(ex));
    }
    if let Some(o) = &ev.organizer {
        m.insert(
            "organizer".into(),
            json!({ "email": o.email, "name": o.name, "is_me": o.is_me }),
        );
    }
    let attendees: Vec<Value> = ev
        .attendees
        .iter()
        .map(|a| {
            json!({
                "email": a.email,
                "name": a.name,
                "status": a.status.as_output(),
                "role": a.role,
                "is_me": a.is_me,
            })
        })
        .collect();
    m.insert("attendees".into(), json!(attendees));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    #[test]
    fn all_day_spans_report_an_inclusive_end_date() {
        let s = When::Date(NaiveDate::from_ymd_opt(2026, 12, 24).unwrap());
        let e = When::Date(NaiveDate::from_ymd_opt(2026, 12, 27).unwrap());
        let m = span_fields(&s, &e, London);
        assert_eq!(m["all_day"], true);
        assert_eq!(m["start_date"], "2026-12-24");
        assert_eq!(m["end_date"], "2026-12-26");
        assert!(m.get("start").is_none());
    }

    #[test]
    fn timed_spans_are_shown_in_the_zone_with_the_event_zone_when_different() {
        let s = When::Zoned {
            local: NaiveDate::from_ymd_opt(2026, 10, 20)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            tzid: "America/New_York".into(),
        };
        let e = s.add(Duration::hours(1)).unwrap();
        let m = span_fields(&s, &e, London);
        assert_eq!(m["start"], "2026-10-20T14:00:00+01:00");
        assert_eq!(m["end"], "2026-10-20T15:00:00+01:00");
        assert_eq!(m["event_timezone"], "America/New_York");
        let same = span_fields(&s, &e, chrono_tz::America::New_York);
        assert_eq!(same["start"], "2026-10-20T09:00:00-04:00");
        assert!(same.get("event_timezone").is_none());
    }

    #[test]
    fn occurrence_values_round_trip() {
        let rid = When::Zoned {
            local: NaiveDate::from_ymd_opt(2026, 11, 2)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            tzid: "Europe/London".into(),
        };
        let out = occurrence_out(&rid, London);
        assert_eq!(out, "2026-11-02T09:00:00+00:00");
        assert_eq!(
            occurrence_in(&out, London).unwrap().instant(London),
            rid.instant(London)
        );
        assert_eq!(
            occurrence_in("2026-11-02", London).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())
        );
    }

    #[test]
    fn timed_input_is_stored_in_the_zone() {
        let w = zoned("2026-07-01T09:00", London).unwrap();
        assert_eq!(
            w,
            When::Zoned {
                local: NaiveDate::from_ymd_opt(2026, 7, 1)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
                tzid: "Europe/London".into()
            }
        );
        let w = zoned("2026-07-01T09:00:00-04:00", London).unwrap();
        assert_eq!(w.value_string(), "20260701T140000");
        let gap = zoned("2026-03-29T01:30", London).unwrap_err();
        assert_eq!(gap.kind(), "validation");
    }

    #[test]
    fn bounds_and_until() {
        assert_eq!(
            bound("2026-07-01", London).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 30, 23, 0, 0).unwrap()
        );
        assert_eq!(
            until_in("2026-12-31", true, London).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap())
        );
        assert_eq!(
            until_in("2026-12-31", false, London).unwrap(),
            When::Utc(Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap())
        );
    }

    #[test]
    fn rrules_are_described_in_words() {
        assert_eq!(describe_rrule("FREQ=DAILY"), "every day");
        assert_eq!(
            describe_rrule("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10"),
            "every 2 weeks on Monday, Wednesday, 10 times"
        );
        assert_eq!(
            describe_rrule("FREQ=MONTHLY;BYDAY=2TU;UNTIL=20261231T235959Z"),
            "every month on 2nd Tuesday, until 2026-12-31"
        );
        assert_eq!(
            describe_rrule("FREQ=MONTHLY;BYDAY=-1FR"),
            "every month on last Friday"
        );
        assert_eq!(
            describe_rrule("FREQ=YEARLY;BYMONTHDAY=15"),
            "every year on day 15"
        );
        assert_eq!(describe_rrule("X=1"), "custom rule X=1");
    }
}
