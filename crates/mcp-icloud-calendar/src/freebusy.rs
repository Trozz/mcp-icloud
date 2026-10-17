//! Free time: working-hours windows minus busy intervals.

use crate::error::{CalError, Result};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;

pub type Interval = (DateTime<Utc>, DateTime<Utc>);

/// Working hours in the active zone, e.g. 09:00 to 17:30 on weekdays.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingHours {
    pub start: NaiveTime,
    pub end: NaiveTime,
    pub days: Vec<Weekday>,
}

impl WorkingHours {
    /// `start`/`end` as `HH:MM`; `days` as `MO`..`SU` (default Monday to Friday).
    pub fn parse(start: &str, end: &str, days: &[String]) -> Result<Self> {
        let time = |s: &str| {
            NaiveTime::parse_from_str(s.trim(), "%H:%M")
                .map_err(|_| CalError::invalid(format!("invalid time {s:?}; use HH:MM")))
        };
        let (start, end) = (time(start)?, time(end)?);
        if end <= start {
            return Err(CalError::invalid(
                "working_hours end must be after its start",
            ));
        }
        let days = if days.is_empty() {
            vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ]
        } else {
            days.iter()
                .map(|d| weekday(d))
                .collect::<Result<Vec<_>>>()?
        };
        Ok(WorkingHours { start, end, days })
    }
}

fn weekday(code: &str) -> Result<Weekday> {
    Ok(match code.trim().to_ascii_uppercase().as_str() {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        other => {
            return Err(CalError::invalid(format!(
                "invalid day {other:?}; use MO, TU, WE, TH, FR, SA or SU"
            )));
        }
    })
}

fn local_instant(day: chrono::NaiveDate, t: NaiveTime, zone: Tz) -> Option<DateTime<Utc>> {
    match zone.from_local_datetime(&day.and_time(t)) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

/// The windows to search: the whole range, or each working day's hours clipped to the range.
pub fn windows(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    zone: Tz,
    hours: Option<&WorkingHours>,
) -> Vec<Interval> {
    let Some(h) = hours else {
        return vec![(start, end)];
    };
    let mut out = Vec::new();
    let mut day = start.with_timezone(&zone).date_naive();
    let last = end.with_timezone(&zone).date_naive();
    while day <= last {
        if h.days.contains(&day.weekday())
            && let (Some(s), Some(e)) = (
                local_instant(day, h.start, zone),
                local_instant(day, h.end, zone),
            )
        {
            let (s, e) = (s.max(start), e.min(end));
            if s < e {
                out.push((s, e));
            }
        }
        day += Duration::days(1);
    }
    out
}

/// Gaps of at least `min` inside `windows` not covered by `busy`.
pub fn free_slots(windows: &[Interval], mut busy: Vec<Interval>, min: Duration) -> Vec<Interval> {
    busy.sort();
    let mut merged: Vec<Interval> = Vec::new();
    for (s, e) in busy {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let mut out = Vec::new();
    for &(ws, we) in windows {
        let mut cursor = ws;
        for &(bs, be) in &merged {
            if be <= cursor || bs >= we {
                continue;
            }
            if bs > cursor && bs - cursor >= min {
                out.push((cursor, bs));
            }
            cursor = cursor.max(be);
            if cursor >= we {
                break;
            }
        }
        if cursor < we && we - cursor >= min {
            out.push((cursor, we));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::London;

    fn t(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
    }

    #[test]
    fn busy_time_is_cut_out_and_short_gaps_dropped() {
        let window = [(t(19, 8, 0), t(19, 16, 0))];
        let busy = vec![
            (t(19, 9, 0), t(19, 10, 0)),
            (t(19, 9, 30), t(19, 10, 15)),
            (t(19, 10, 30), t(19, 11, 0)),
            (t(19, 15, 45), t(19, 17, 0)),
        ];
        let free = free_slots(&window, busy, Duration::minutes(30));
        assert_eq!(
            free,
            [(t(19, 8, 0), t(19, 9, 0)), (t(19, 11, 0), t(19, 15, 45)),]
        );
    }

    #[test]
    fn whole_window_is_free_without_busy_time() {
        let window = [(t(19, 8, 0), t(19, 9, 0))];
        assert_eq!(
            free_slots(&window, vec![], Duration::minutes(60)),
            window.to_vec()
        );
        assert!(free_slots(&window, vec![], Duration::minutes(61)).is_empty());
    }

    #[test]
    fn working_hours_follow_the_local_clock_across_dst() {
        let h = WorkingHours::parse("09:00", "17:00", &[]).unwrap();
        // Fri 23 Oct to Tue 27 Oct 2026; clocks go back on Sun 25 Oct.
        let w = windows(t(23, 0, 0), t(27, 23, 0), London, Some(&h));
        assert_eq!(
            w,
            [
                (t(23, 8, 0), t(23, 16, 0)),
                (t(26, 9, 0), t(26, 17, 0)),
                (t(27, 9, 0), t(27, 17, 0)),
            ]
        );
    }

    #[test]
    fn windows_are_clipped_to_the_range() {
        let h = WorkingHours::parse("09:00", "17:00", &["MO".into()]).unwrap();
        let w = windows(t(26, 12, 0), t(26, 14, 0), London, Some(&h));
        assert_eq!(w, [(t(26, 12, 0), t(26, 14, 0))]);
        assert_eq!(
            windows(t(26, 12, 0), t(26, 14, 0), London, None),
            [(t(26, 12, 0), t(26, 14, 0))]
        );
    }

    #[test]
    fn bad_working_hours_are_rejected() {
        assert!(WorkingHours::parse("17:00", "09:00", &[]).is_err());
        assert!(WorkingHours::parse("9am", "17:00", &[]).is_err());
        assert!(WorkingHours::parse("09:00", "17:00", &["XX".into()]).is_err());
    }
}
