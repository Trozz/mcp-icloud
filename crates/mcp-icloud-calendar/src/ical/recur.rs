//! Recurrence: expanding a resource into occurrences, and editing single occurrences or the rest
//! of a series. Expansion uses the `rrule` crate for RRULE only; RDATE, EXDATE and RECURRENCE-ID
//! overrides are applied here.

use crate::error::{CalError, Result};
use crate::ical::build::{PRODID, stamp, touch};
use crate::ical::model::{EventData, MyAddresses, When};
use crate::ical::parse::{event_from_component, master, master_mut};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use mcp_icloud_dav::line::{Component, Property};

pub const MAX_OCCURRENCES: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    /// The original start of this occurrence (RECURRENCE-ID); `None` for a non-recurring event.
    pub recurrence_id: Option<When>,
    pub start: When,
    pub end: When,
    /// The master's data, or the override's for a changed occurrence.
    pub event: EventData,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expansion {
    pub occurrences: Vec<Occurrence>,
    pub truncated: bool,
}

/// Occurrences that overlap `[range_start, range_end)`, sorted by start, at most `limit`.
pub fn expand(
    cal: &Component,
    me: &MyAddresses,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    fallback: Tz,
    limit: usize,
) -> Result<Expansion> {
    let master_data = master(cal)
        .map(|m| event_from_component(m, me))
        .transpose()?;
    let overrides: Vec<EventData> = cal
        .children("VEVENT")
        .filter(|c| c.prop("RECURRENCE-ID").is_some())
        .map(|c| event_from_component(c, me))
        .collect::<Result<_>>()?;
    let overlaps = |s: &When, e: &When| {
        let (s, e) = (s.instant(fallback), e.instant(fallback));
        s < range_end && (e > range_start || (s == e && s >= range_start))
    };
    let mut out: Vec<Occurrence> = Vec::new();
    let mut limited = false;
    if let Some(m) = &master_data {
        if !m.is_recurring() {
            if overlaps(&m.start, &m.end) {
                out.push(Occurrence {
                    recurrence_id: None,
                    start: m.start.clone(),
                    end: m.end.clone(),
                    event: m.clone(),
                });
            }
        } else {
            let duration = m.duration(fallback);
            let overridden: Vec<DateTime<Utc>> = overrides
                .iter()
                .filter_map(|o| o.recurrence_id.as_ref())
                .map(|r| r.instant(fallback))
                .collect();
            let (instants, was_limited) =
                series_instants(m, range_start - duration, range_end, fallback, true);
            limited = was_limited;
            for t in instants {
                if overridden.contains(&t) {
                    continue;
                }
                let start = When::in_style_of(t, &m.start, fallback);
                let Some(end) = start.add(duration) else {
                    continue;
                };
                if overlaps(&start, &end) {
                    out.push(Occurrence {
                        recurrence_id: Some(start.clone()),
                        start,
                        end,
                        event: m.clone(),
                    });
                }
            }
        }
    }
    for o in overrides {
        if o.is_cancelled() || !overlaps(&o.start, &o.end) {
            continue;
        }
        if let Some(m) = &master_data
            && o.recurrence_id.as_ref().is_some_and(|r| {
                m.exdates
                    .iter()
                    .any(|x| x.instant(fallback) == r.instant(fallback))
            })
        {
            continue;
        }
        out.push(Occurrence {
            recurrence_id: o.recurrence_id.clone(),
            start: o.start.clone(),
            end: o.end.clone(),
            event: o,
        });
    }
    out.sort_by_key(|o| o.start.instant(fallback));
    let truncated = limited || out.len() > limit;
    out.truncate(limit);
    Ok(Expansion {
        occurrences: out,
        truncated,
    })
}

/// Series start instants in `[from, to]` (RRULE plus RDATE, minus EXDATE when asked). Returns
/// whether the rrule crate stopped early. A rule the crate rejects yields only DTSTART.
fn series_instants(
    m: &EventData,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    fallback: Tz,
    apply_exdates: bool,
) -> (Vec<DateTime<Utc>>, bool) {
    let first = m.start.instant(fallback);
    let mut limited = false;
    let mut out: Vec<DateTime<Utc>> = match &m.rrule {
        None => vec![first],
        Some(rule) => match rrule_dates(&m.start, rule, from, to, fallback) {
            Ok((dates, was_limited)) => {
                limited = was_limited;
                dates
            }
            Err(e) => {
                tracing::warn!(uid = %m.uid, error = %e, "unsupported RRULE, showing the first occurrence only");
                vec![first]
            }
        },
    };
    out.extend(m.rdates.iter().map(|r| r.instant(fallback)));
    out.retain(|t| *t >= from && *t <= to);
    if apply_exdates {
        let ex: Vec<DateTime<Utc>> = m.exdates.iter().map(|x| x.instant(fallback)).collect();
        out.retain(|t| !ex.contains(t));
    }
    out.sort();
    out.dedup();
    (out, limited)
}

fn rrule_dates(
    start: &When,
    rule: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    fallback: Tz,
) -> std::result::Result<(Vec<DateTime<Utc>>, bool), String> {
    let zone = start.zone(fallback);
    let dtstart = match start {
        When::Utc(dt) => format!("DTSTART:{}", When::Utc(*dt).value_string()),
        When::Date(d) => format!("DTSTART;TZID={}:{}T000000", zone.name(), d.format("%Y%m%d")),
        When::Zoned { local, .. } | When::Floating(local) => format!(
            "DTSTART;TZID={}:{}",
            zone.name(),
            local.format("%Y%m%dT%H%M%S")
        ),
    };
    let rule = normalize_until(rule, zone);
    let set: rrule::RRuleSet = format!("{dtstart}\nRRULE:{rule}")
        .parse()
        .map_err(|e: rrule::RRuleError| e.to_string())?;
    let rtz = rrule::Tz::Tz(chrono_tz::UTC);
    let result = set
        .after(from.with_timezone(&rtz))
        .before(to.with_timezone(&rtz))
        .all(u16::MAX);
    // `limited` is also set when the crate's own iteration guard stopped it.
    let limited = result.limited && result.dates.len() >= usize::from(u16::MAX);
    let dates = result
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect();
    Ok((dates, limited))
}

/// Rewrites UNTIL as UTC (the rrule crate requires that for zoned starts). A DATE UNTIL means
/// the end of that day in `zone`; a local UNTIL is read in `zone`.
fn normalize_until(rule: &str, zone: Tz) -> String {
    let parts: Vec<String> = rule
        .split(';')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let Some((k, v)) = p.split_once('=') else {
                return p.to_string();
            };
            if !k.eq_ignore_ascii_case("UNTIL") {
                return p.to_string();
            }
            let local = if v.len() == 8 {
                format!("{v}T235959")
            } else {
                v.trim_end_matches(['Z', 'z']).to_string()
            };
            if v.ends_with(['Z', 'z']) {
                return format!("UNTIL={}Z", local);
            }
            let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&local, "%Y%m%dT%H%M%S") else {
                return p.to_string();
            };
            let utc = When::Zoned {
                local: naive,
                tzid: zone.name().to_string(),
            }
            .instant(zone);
            format!("UNTIL={}", When::Utc(utc).value_string())
        })
        .collect();
    parts.join(";")
}

fn end_of(start: &When, m: &EventData, fallback: Tz) -> Result<When> {
    start
        .add(m.duration(fallback))
        .ok_or_else(|| CalError::invalid("date out of range"))
}

fn series_master(cal: &Component) -> Result<EventData> {
    let m = master(cal).ok_or_else(|| {
        CalError::invalid("this resource has no series master; edit the occurrence itself")
    })?;
    let data = event_from_component(m, &MyAddresses::default())?;
    if !data.is_recurring() {
        return Err(CalError::invalid(
            "this event is not recurring; use scope series",
        ));
    }
    Ok(data)
}

/// The occurrence whose original start is `original` (a date, or any instant), written in the
/// series' own style. A validation error when the series has no such occurrence.
pub fn find_occurrence(cal: &Component, original: &When, fallback: Tz) -> Result<When> {
    let m = series_master(cal)?;
    let target = original.instant(fallback);
    let not_found = || {
        CalError::invalid(
            "no occurrence of this event starts at that time; use occurrence_start from list_events",
        )
    };
    for o in cal
        .children("VEVENT")
        .filter_map(|c| c.prop("RECURRENCE-ID"))
    {
        let rid = When::from_property(o)?;
        if same_occurrence(&rid, original, target, fallback) {
            return Ok(rid);
        }
    }
    let window = if original.is_date() {
        Duration::days(1)
    } else {
        Duration::seconds(1)
    };
    let (instants, _) = series_instants(&m, target - window, target + window, fallback, true);
    let hit = instants.into_iter().find(|t| {
        let candidate = When::in_style_of(*t, &m.start, fallback);
        same_occurrence(&candidate, original, target, fallback)
    });
    hit.map(|t| When::in_style_of(t, &m.start, fallback))
        .ok_or_else(not_found)
}

fn same_occurrence(candidate: &When, original: &When, target: DateTime<Utc>, fallback: Tz) -> bool {
    match (candidate, original) {
        (When::Date(a), When::Date(b)) => a == b,
        (_, When::Date(b)) => {
            candidate
                .instant(fallback)
                .with_timezone(&fallback)
                .date_naive()
                == *b
        }
        _ => candidate.instant(fallback) == target,
    }
}

/// The override VEVENT for `rid`, created from the master when it does not exist yet.
pub fn override_mut<'a>(
    cal: &'a mut Component,
    rid: &When,
    fallback: Tz,
) -> Result<&'a mut Component> {
    let target = rid.instant(fallback);
    let existing = cal.components.iter().position(|c| {
        c.name.eq_ignore_ascii_case("VEVENT")
            && c.prop("RECURRENCE-ID")
                .and_then(|p| When::from_property(p).ok())
                .is_some_and(|w| w.instant(fallback) == target)
    });
    if let Some(i) = existing {
        return Ok(&mut cal.components[i]);
    }
    let m = series_master(cal)?;
    let template = master(cal)
        .cloned()
        .ok_or_else(|| CalError::Internal("master vanished".into()))?;
    let mut o = template;
    for name in ["RRULE", "RDATE", "EXDATE", "EXRULE", "DURATION"] {
        o.remove(name);
    }
    let start = When::in_style_of(target, &m.start, fallback);
    o.set(start.to_property("DTSTART"));
    o.set(end_of(&start, &m, fallback)?.to_property("DTEND"));
    o.push(start.to_property("RECURRENCE-ID"));
    cal.components.push(o);
    let last = cal.components.len() - 1;
    Ok(&mut cal.components[last])
}

/// Removes one occurrence: adds an EXDATE to the master and drops its override.
pub fn exclude_occurrence(
    cal: &mut Component,
    rid: &When,
    now: DateTime<Utc>,
    fallback: Tz,
) -> Result<()> {
    let m = series_master(cal)?;
    let target = rid.instant(fallback);
    remove_overrides(cal, fallback, |t| t == target);
    let ex = When::in_style_of(target, &m.start, fallback);
    let master = master_mut(cal).ok_or_else(|| CalError::Internal("master vanished".into()))?;
    master.push(ex.to_property("EXDATE"));
    touch(master, now, true);
    Ok(())
}

fn remove_overrides(cal: &mut Component, fallback: Tz, drop: impl Fn(DateTime<Utc>) -> bool) {
    cal.components.retain(|c| {
        let rid = c
            .prop("RECURRENCE-ID")
            .and_then(|p| When::from_property(p).ok());
        match rid {
            Some(r) if c.name.eq_ignore_ascii_case("VEVENT") => !drop(r.instant(fallback)),
            _ => true,
        }
    });
}

/// Ends the series just before `rid` and drops later overrides, EXDATEs and RDATEs. Returns
/// whether any occurrence remains (false means the caller should delete the resource).
pub fn truncate_series(
    cal: &mut Component,
    rid: &When,
    now: DateTime<Utc>,
    fallback: Tz,
) -> Result<bool> {
    let m = series_master(cal)?;
    let target = rid.instant(fallback);
    let remaining = m.start.instant(fallback) < target;
    remove_overrides(cal, fallback, |t| t >= target);
    let master = master_mut(cal).ok_or_else(|| CalError::Internal("master vanished".into()))?;
    if let Some(rule) = master.value("RRULE").map(str::to_string) {
        let until = if m.start.is_date() {
            When::in_style_of(target, &m.start, fallback)
                .add(Duration::days(-1))
                .ok_or_else(|| CalError::invalid("date out of range"))?
                .value_string()
        } else {
            When::Utc(target - Duration::seconds(1)).value_string()
        };
        let mut parts: Vec<String> = rule
            .split(';')
            .filter(|p| {
                let key = p.split('=').next().unwrap_or_default();
                !key.eq_ignore_ascii_case("UNTIL") && !key.eq_ignore_ascii_case("COUNT")
            })
            .map(str::to_string)
            .collect();
        parts.push(format!("UNTIL={until}"));
        master.set(Property::new("RRULE", parts.join(";")));
    }
    retain_dates(master, "EXDATE", fallback, |t| t < target);
    retain_dates(master, "RDATE", fallback, |t| t < target);
    touch(master, now, true);
    Ok(remaining)
}

/// Keeps only the values of a date-list property (EXDATE, RDATE) that pass `keep`.
fn retain_dates(c: &mut Component, name: &str, fallback: Tz, keep: impl Fn(DateTime<Utc>) -> bool) {
    let mut rebuilt: Vec<Property> = Vec::new();
    for p in c.props(name) {
        let Ok(values) = crate::ical::model::when_list(p) else {
            rebuilt.push(p.clone());
            continue;
        };
        let total = values.len();
        let kept: Vec<When> = values
            .into_iter()
            .filter(|w| keep(w.instant(fallback)))
            .collect();
        if kept.len() == total {
            rebuilt.push(p.clone());
            continue;
        }
        for w in kept {
            rebuilt.push(w.to_property(name));
        }
    }
    c.remove(name);
    for p in rebuilt {
        c.push(p);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// False when the split was at the first occurrence: the old resource should be deleted.
    pub old_has_occurrences: bool,
    /// A new VCALENDAR holding the series from `rid` on, with `new_uid`.
    pub new_series: Component,
}

/// "This and following": ends the old series before `rid` and returns a new series (new UID)
/// that starts at `rid` with the same rule, the remaining COUNT, and the later overrides.
pub fn split_series(
    cal: &mut Component,
    rid: &When,
    new_uid: &str,
    now: DateTime<Utc>,
    fallback: Tz,
) -> Result<Split> {
    let m = series_master(cal)?;
    let target = rid.instant(fallback);
    let template = master(cal)
        .cloned()
        .ok_or_else(|| CalError::Internal("master vanished".into()))?;
    let mut new_master = template;
    let start = When::in_style_of(target, &m.start, fallback);
    new_master.set(Property::new("UID", new_uid));
    new_master.set(start.to_property("DTSTART"));
    new_master.remove("DURATION");
    new_master.set(end_of(&start, &m, fallback)?.to_property("DTEND"));
    if let Some(rule) = &m.rrule {
        new_master.set(Property::new(
            "RRULE",
            remaining_rule(&m, rule, target, fallback),
        ));
    }
    retain_dates(&mut new_master, "EXDATE", fallback, |t| t >= target);
    retain_dates(&mut new_master, "RDATE", fallback, |t| t >= target);
    new_master.set(Property::new("SEQUENCE", "0"));
    new_master.set(Property::new("CREATED", stamp(now)));
    touch(&mut new_master, now, false);

    let mut new_cal = Component::new("VCALENDAR");
    new_cal.push(Property::new("VERSION", "2.0"));
    new_cal.push(Property::new("PRODID", PRODID));
    new_cal.push(Property::new("CALSCALE", "GREGORIAN"));
    new_cal
        .components
        .extend(cal.children("VTIMEZONE").cloned());
    new_cal.components.push(new_master);
    for c in cal.children("VEVENT") {
        let Some(r) = c
            .prop("RECURRENCE-ID")
            .and_then(|p| When::from_property(p).ok())
        else {
            continue;
        };
        if r.instant(fallback) >= target {
            let mut moved = c.clone();
            moved.set(Property::new("UID", new_uid));
            new_cal.components.push(moved);
        }
    }
    let old_has_occurrences = truncate_series(cal, rid, now, fallback)?;
    Ok(Split {
        old_has_occurrences,
        new_series: new_cal,
    })
}

/// The rule for the new series: COUNT reduced by the occurrences before `target`.
fn remaining_rule(m: &EventData, rule: &str, target: DateTime<Utc>, fallback: Tz) -> String {
    let count = rule.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.eq_ignore_ascii_case("COUNT")
            .then(|| v.parse::<u32>().ok())?
    });
    let Some(count) = count else {
        return rule.to_string();
    };
    let first = m.start.instant(fallback);
    let (before, _) = series_instants(m, first, target - Duration::seconds(1), fallback, false);
    let left = count.saturating_sub(before.len() as u32).max(1);
    rule.split(';')
        .map(|p| match p.split_once('=') {
            Some((k, _)) if k.eq_ignore_ascii_case("COUNT") => format!("COUNT={left}"),
            _ => p.to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ical::build::{Freq, NewEvent, Recurrence, new_resource};
    use crate::ical::parse::{events, parse_resource};
    use chrono::{NaiveDate, NaiveDateTime};
    use chrono_tz::Europe;

    const WEEKLY: &str = include_str!("../../tests/fixtures/apple-weekly.ics");
    const OVERRIDE: &str = include_str!("../../tests/fixtures/master-override.ics");

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        utc("2026-10-17T12:00:00Z")
    }

    fn zoned(s: &str) -> When {
        When::Zoned {
            local: NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap(),
            tzid: "Europe/London".into(),
        }
    }

    fn me() -> MyAddresses {
        MyAddresses::new(["mailto:user@example.com"])
    }

    fn starts(cal: &Component, from: &str, to: &str) -> Vec<String> {
        expand(
            cal,
            &me(),
            utc(from),
            utc(to),
            Europe::London,
            MAX_OCCURRENCES,
        )
        .unwrap()
        .occurrences
        .iter()
        .map(|o| o.start.instant(Europe::London).to_rfc3339())
        .collect()
    }

    #[test]
    fn weekly_series_keeps_local_time_across_dst() {
        let cal = parse_resource(WEEKLY).unwrap();
        let got = starts(&cal, "2026-03-16T00:00:00Z", "2026-04-07T00:00:00Z");
        assert_eq!(
            got,
            [
                "2026-03-16T09:00:00+00:00",
                "2026-03-23T09:00:00+00:00",
                "2026-03-30T08:00:00+00:00",
                "2026-04-06T08:00:00+00:00",
            ]
        );
        let got = starts(&cal, "2026-10-19T00:00:00Z", "2026-11-03T00:00:00Z");
        assert_eq!(
            got,
            [
                "2026-10-19T08:00:00+00:00",
                "2026-10-26T09:00:00+00:00",
                "2026-11-02T09:00:00+00:00",
            ]
        );
    }

    #[test]
    fn overrides_move_and_cancel_and_exdates_remove() {
        let cal = parse_resource(OVERRIDE).unwrap();
        let exp = expand(
            &cal,
            &me(),
            utc("2026-01-01T00:00:00Z"),
            utc("2026-03-01T00:00:00Z"),
            Europe::London,
            MAX_OCCURRENCES,
        )
        .unwrap();
        let summary: Vec<(String, String)> = exp
            .occurrences
            .iter()
            .map(|o| {
                (
                    o.start.instant(Europe::London).to_rfc3339(),
                    o.event.summary.clone().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("2026-01-05T10:00:00+00:00".into(), "Planning".into()),
                (
                    "2026-01-12T14:00:00+00:00".into(),
                    "Planning (moved)".into()
                ),
                ("2026-02-02T10:00:00+00:00".into(), "Planning".into()),
                ("2026-02-09T10:00:00+00:00".into(), "Planning".into()),
            ]
        );
        assert!(!exp.truncated);
        let moved = &exp.occurrences[1];
        assert_eq!(moved.recurrence_id, Some(zoned("2026-01-12T10:00")));
    }

    #[test]
    fn an_override_moved_out_of_the_range_leaves_no_ghost() {
        let cal = parse_resource(OVERRIDE).unwrap();
        let got = starts(&cal, "2026-01-12T09:00:00Z", "2026-01-12T12:00:00Z");
        assert!(got.is_empty(), "{got:?}");
        let got = starts(&cal, "2026-01-12T13:30:00Z", "2026-01-12T14:30:00Z");
        assert_eq!(got, ["2026-01-12T14:00:00+00:00"]);
    }

    #[test]
    fn long_daily_series_is_capped() {
        let ev = NewEvent {
            summary: "Daily".into(),
            start: zoned("2026-01-01T07:00"),
            end: zoned("2026-01-01T07:15"),
            location: None,
            description: None,
            recurrence: Some(Recurrence {
                freq: Freq::Daily,
                interval: None,
                by_day: vec![],
                by_month_day: vec![],
                until: None,
                count: None,
            }),
            alerts_minutes_before: vec![],
            attendees: vec![],
            organizer_email: None,
        };
        let cal = new_resource(&ev, "daily@example.com", now(), Europe::London).unwrap();
        let exp = expand(
            &cal,
            &me(),
            utc("2026-01-01T00:00:00Z"),
            utc("2027-12-31T00:00:00Z"),
            Europe::London,
            MAX_OCCURRENCES,
        )
        .unwrap();
        assert_eq!(exp.occurrences.len(), 500);
        assert!(exp.truncated);
    }

    #[test]
    fn single_events_and_partial_overlap() {
        let cal = parse_resource(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:s\nDTSTART:20261020T130000Z\nDTEND:20261020T140000Z\nEND:VEVENT\nEND:VCALENDAR\n",
        )
        .unwrap();
        assert_eq!(
            starts(&cal, "2026-10-20T13:30:00Z", "2026-10-21T00:00:00Z").len(),
            1
        );
        assert!(starts(&cal, "2026-10-20T14:00:00Z", "2026-10-21T00:00:00Z").is_empty());
        let exp = expand(
            &cal,
            &me(),
            utc("2026-10-20T00:00:00Z"),
            utc("2026-10-21T00:00:00Z"),
            Europe::London,
            10,
        )
        .unwrap();
        assert_eq!(exp.occurrences[0].recurrence_id, None);
    }

    #[test]
    fn all_day_yearly_series_expands_as_dates() {
        let cal = parse_resource(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:b\nDTSTART;VALUE=DATE:20200314\nDTEND;VALUE=DATE:20200315\nRRULE:FREQ=YEARLY;UNTIL=20280314\nSUMMARY:Birthday\nEND:VEVENT\nEND:VCALENDAR\n",
        )
        .unwrap();
        let exp = expand(
            &cal,
            &me(),
            utc("2026-01-01T00:00:00Z"),
            utc("2030-01-01T00:00:00Z"),
            Europe::London,
            10,
        )
        .unwrap();
        let dates: Vec<String> = exp
            .occurrences
            .iter()
            .map(|o| o.start.value_string())
            .collect();
        assert_eq!(dates, ["20260314", "20270314", "20280314"]);
        assert!(exp.occurrences.iter().all(|o| o.end.is_date()));
    }

    #[test]
    fn find_occurrence_matches_series_style_or_fails() {
        let cal = parse_resource(OVERRIDE).unwrap();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-02-02T10:00:00Z")),
            Europe::London,
        )
        .unwrap();
        assert_eq!(rid, zoned("2026-02-02T10:00"));
        let moved = find_occurrence(
            &cal,
            &When::Utc(utc("2026-01-12T10:00:00Z")),
            Europe::London,
        )
        .unwrap();
        assert_eq!(moved, zoned("2026-01-12T10:00"));
        let by_date = find_occurrence(
            &cal,
            &When::Date(NaiveDate::from_ymd_opt(2026, 2, 9).unwrap()),
            Europe::London,
        )
        .unwrap();
        assert_eq!(by_date, zoned("2026-02-09T10:00"));
        for missing in [
            "2026-01-19T10:00:00Z",
            "2026-02-02T11:00:00Z",
            "2026-03-30T10:00:00Z",
        ] {
            let e = find_occurrence(&cal, &When::Utc(utc(missing)), Europe::London).unwrap_err();
            assert_eq!(e.kind(), "validation", "{missing}");
            assert!(e.to_string().contains("list_events"));
        }
        let single = parse_resource(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:s\nDTSTART:20261020T130000Z\nEND:VEVENT\nEND:VCALENDAR\n",
        )
        .unwrap();
        let e = find_occurrence(
            &single,
            &When::Utc(utc("2026-10-20T13:00:00Z")),
            Europe::London,
        )
        .unwrap_err();
        assert!(e.to_string().contains("not recurring"));
    }

    #[test]
    fn editing_one_occurrence_creates_an_override() {
        let mut cal = parse_resource(WEEKLY).unwrap();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-03-30T08:00:00Z")),
            Europe::London,
        )
        .unwrap();
        let o = override_mut(&mut cal, &rid, Europe::London).unwrap();
        o.set(Property::new("SUMMARY", "Stand-up (long)"));
        let ics = cal.to_ics();
        assert!(ics.contains("RECURRENCE-ID;TZID=Europe/London:20260330T090000\r\n"));
        let again = override_mut(&mut cal, &rid, Europe::London).unwrap();
        assert_eq!(again.value("SUMMARY"), Some("Stand-up (long)"));
        assert_eq!(cal.children("VEVENT").count(), 2);
        let exp = expand(
            &cal,
            &me(),
            utc("2026-03-30T00:00:00Z"),
            utc("2026-03-31T00:00:00Z"),
            Europe::London,
            10,
        )
        .unwrap();
        assert_eq!(exp.occurrences.len(), 1);
        assert_eq!(
            exp.occurrences[0].event.summary.as_deref(),
            Some("Stand-up (long)")
        );
        let o = cal.children("VEVENT").nth(1).unwrap();
        assert!(o.prop("RRULE").is_none());
        assert_eq!(o.value("DTEND"), Some("20260330T093000"));
    }

    #[test]
    fn excluding_an_occurrence_adds_exdate_and_drops_the_override() {
        let mut cal = parse_resource(OVERRIDE).unwrap();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-01-12T10:00:00Z")),
            Europe::London,
        )
        .unwrap();
        exclude_occurrence(&mut cal, &rid, now(), Europe::London).unwrap();
        assert!(
            cal.to_ics()
                .contains("EXDATE;TZID=Europe/London:20260112T100000\r\n")
        );
        let got = starts(&cal, "2026-01-01T00:00:00Z", "2026-03-01T00:00:00Z");
        assert_eq!(
            got,
            [
                "2026-01-05T10:00:00+00:00",
                "2026-02-02T10:00:00+00:00",
                "2026-02-09T10:00:00+00:00"
            ]
        );
    }

    #[test]
    fn this_and_following_split_preserves_the_occurrences() {
        let original = parse_resource(OVERRIDE).unwrap();
        let before = starts(&original, "2026-01-01T00:00:00Z", "2026-03-01T00:00:00Z");
        let mut cal = original.clone();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-02-02T10:00:00Z")),
            Europe::London,
        )
        .unwrap();
        let split =
            split_series(&mut cal, &rid, "new-uid@example.com", now(), Europe::London).unwrap();
        assert!(split.old_has_occurrences);
        let old = starts(&cal, "2026-01-01T00:00:00Z", "2026-03-01T00:00:00Z");
        let new = starts(
            &split.new_series,
            "2026-01-01T00:00:00Z",
            "2026-03-01T00:00:00Z",
        );
        assert_eq!(old, before[..2]);
        assert_eq!(new, before[2..]);
        let new_master = master(&split.new_series).unwrap();
        assert_eq!(new_master.value("UID"), Some("new-uid@example.com"));
        assert_eq!(new_master.value("RRULE"), Some("FREQ=WEEKLY;COUNT=2"));
        assert!(
            split
                .new_series
                .to_ics()
                .contains("PRODID:-//Trozz//mcp-icloud//EN")
        );
        let old_rule = master(&cal).unwrap().value("RRULE").unwrap();
        assert_eq!(old_rule, "FREQ=WEEKLY;UNTIL=20260202T095959Z");
        assert_eq!(
            events(&cal, &me()).unwrap().len(),
            3,
            "overrides before the split stay with the old series"
        );
        assert_eq!(events(&split.new_series, &me()).unwrap().len(), 1);
    }

    #[test]
    fn later_overrides_move_to_the_new_series() {
        let mut cal = parse_resource(OVERRIDE).unwrap();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-01-12T10:00:00Z")),
            Europe::London,
        )
        .unwrap();
        let split =
            split_series(&mut cal, &rid, "new-uid@example.com", now(), Europe::London).unwrap();
        assert_eq!(events(&cal, &me()).unwrap().len(), 1);
        let moved = events(&split.new_series, &me()).unwrap();
        assert_eq!(moved.len(), 3);
        assert!(moved.iter().all(|e| e.uid == "new-uid@example.com"));
        assert_eq!(
            master(&split.new_series).unwrap().value("EXDATE"),
            Some("20260119T100000")
        );
        let new = starts(
            &split.new_series,
            "2026-01-01T00:00:00Z",
            "2026-03-01T00:00:00Z",
        );
        assert_eq!(
            new,
            [
                "2026-01-12T14:00:00+00:00",
                "2026-02-02T10:00:00+00:00",
                "2026-02-09T10:00:00+00:00"
            ]
        );
    }

    #[test]
    fn split_at_the_first_occurrence_leaves_nothing_behind() {
        let mut cal = parse_resource(WEEKLY).unwrap();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-03-02T09:00:00Z")),
            Europe::London,
        )
        .unwrap();
        let split = split_series(&mut cal, &rid, "n@example.com", now(), Europe::London).unwrap();
        assert!(!split.old_has_occurrences);
        let rule = master(&split.new_series).unwrap().value("RRULE").unwrap();
        assert_eq!(rule, "FREQ=WEEKLY;BYDAY=MO");
    }

    #[test]
    fn truncating_keeps_earlier_occurrences_only() {
        let mut cal = parse_resource(WEEKLY).unwrap();
        let rid = find_occurrence(
            &cal,
            &When::Utc(utc("2026-03-23T09:00:00Z")),
            Europe::London,
        )
        .unwrap();
        assert!(truncate_series(&mut cal, &rid, now(), Europe::London).unwrap());
        let got = starts(&cal, "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z");
        assert_eq!(
            got,
            [
                "2026-03-02T09:00:00+00:00",
                "2026-03-09T09:00:00+00:00",
                "2026-03-16T09:00:00+00:00"
            ]
        );
    }

    #[test]
    fn local_and_date_until_values_are_normalized() {
        assert_eq!(
            normalize_until("FREQ=WEEKLY;UNTIL=20260701T090000", Europe::London),
            "FREQ=WEEKLY;UNTIL=20260701T080000Z"
        );
        assert_eq!(
            normalize_until("FREQ=DAILY;UNTIL=20261231", Europe::London),
            "FREQ=DAILY;UNTIL=20261231T235959Z"
        );
        assert_eq!(
            normalize_until("FREQ=DAILY;UNTIL=20261231T120000Z;COUNT=2", Europe::London),
            "FREQ=DAILY;UNTIL=20261231T120000Z;COUNT=2"
        );
    }
}
