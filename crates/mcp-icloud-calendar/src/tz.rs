use crate::error::{CalError, Result};
use chrono::{DateTime, LocalResult, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneSource {
    Config,
    Override,
}

impl ZoneSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ZoneSource::Config => "config",
            ZoneSource::Override => "override",
        }
    }
}

/// The active time zone: the configured default, optionally replaced in memory by `set_timezone`.
pub struct ZoneState {
    default: Tz,
    current: RwLock<Option<Tz>>,
}

impl ZoneState {
    pub fn new(default: Tz) -> Self {
        Self {
            default,
            current: RwLock::new(None),
        }
    }

    pub fn active(&self) -> Tz {
        self.read().unwrap_or(self.default)
    }

    pub fn source(&self) -> ZoneSource {
        match self.read() {
            Some(_) => ZoneSource::Override,
            None => ZoneSource::Config,
        }
    }

    pub fn default_zone(&self) -> Tz {
        self.default
    }

    /// `Some` sets the override, `None` clears it.
    pub fn set(&self, zone: Option<Tz>) {
        let mut guard = self.current.write().unwrap_or_else(|e| e.into_inner());
        *guard = zone;
    }

    fn read(&self) -> Option<Tz> {
        *self.current.read().unwrap_or_else(|e| e.into_inner())
    }
}

pub fn parse_zone(name: &str) -> Result<Tz> {
    name.trim().parse::<Tz>().map_err(|_| {
        CalError::invalid(format!(
            "unknown time zone {name:?}; use an IANA name such as Europe/London or America/New_York"
        ))
    })
}

/// Resolves a wall-clock time in `tz`. A time skipped by a DST change is an error; a repeated
/// time resolves to the earlier instant.
pub fn resolve_local(naive: NaiveDateTime, tz: Tz) -> Result<DateTime<Utc>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Ok(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(earlier, _) => Ok(earlier.with_timezone(&Utc)),
        LocalResult::None => Err(CalError::invalid(format!(
            "{} does not exist in {}: the clocks change at that time (daylight saving gap); choose a time outside the gap",
            naive.format("%Y-%m-%d %H:%M"),
            tz.name()
        ))),
    }
}

/// RFC 3339 with an offset is used as given. Without an offset (`2026-10-17T09:00`,
/// `2026-10-17T09:00:00`, `2026-10-17 09:00`) the time is read in `tz`.
pub fn parse_datetime(input: &str, tz: Tz) -> Result<DateTime<Utc>> {
    let s = input.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    let naive = parse_naive(s).ok_or_else(|| {
        CalError::invalid(format!(
            "invalid date-time {input:?}; use RFC 3339 such as 2026-10-17T09:00:00+01:00, or 2026-10-17T09:00 for the active time zone"
        ))
    })?;
    resolve_local(naive, tz)
}

pub fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
}

pub fn parse_date(input: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(input.trim(), "%Y-%m-%d")
        .map_err(|_| CalError::invalid(format!("invalid date {input:?}; use YYYY-MM-DD")))
}

/// RFC 3339 in `tz`, whole seconds, numeric offset.
pub fn format_in(dt: DateTime<Utc>, tz: Tz) -> String {
    dt.with_timezone(&tz)
        .to_rfc3339_opts(SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::{America, Europe};

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn override_replaces_and_clears() {
        let z = ZoneState::new(Europe::London);
        assert_eq!(z.active(), Europe::London);
        assert_eq!(z.source(), ZoneSource::Config);
        z.set(Some(America::New_York));
        assert_eq!(z.active(), America::New_York);
        assert_eq!(z.source().as_str(), "override");
        z.set(None);
        assert_eq!(z.active(), Europe::London);
        assert_eq!(z.source().as_str(), "config");
        assert_eq!(z.default_zone(), Europe::London);
    }

    #[test]
    fn invalid_zone_is_a_validation_error() {
        assert_eq!(parse_zone("America/New_York").unwrap(), America::New_York);
        let e = parse_zone("New York").unwrap_err();
        assert_eq!(e.kind(), "validation");
        assert!(e.to_string().contains("IANA"));
    }

    #[test]
    fn offset_input_is_used_as_given() {
        let dt = parse_datetime("2026-07-01T09:00:00-04:00", Europe::London).unwrap();
        assert_eq!(dt, utc("2026-07-01T13:00:00Z"));
    }

    #[test]
    fn naive_input_is_read_in_the_zone() {
        assert_eq!(
            parse_datetime("2026-07-01T09:00", Europe::London).unwrap(),
            utc("2026-07-01T08:00:00Z")
        );
        assert_eq!(
            parse_datetime("2026-12-01 09:00", Europe::London).unwrap(),
            utc("2026-12-01T09:00:00Z")
        );
        assert_eq!(
            parse_datetime("2026-07-01T09:00:30", America::New_York).unwrap(),
            utc("2026-07-01T13:00:30Z")
        );
    }

    #[test]
    fn dst_gap_is_rejected_with_a_message() {
        let e = parse_datetime("2026-03-29T01:30", Europe::London).unwrap_err();
        assert_eq!(e.kind(), "validation");
        let msg = e.to_string();
        assert!(msg.contains("2026-03-29 01:30"), "{msg}");
        assert!(msg.contains("daylight saving gap"), "{msg}");
    }

    #[test]
    fn dst_overlap_resolves_to_the_earlier_instant() {
        let dt = parse_datetime("2026-10-25T01:30", Europe::London).unwrap();
        assert_eq!(dt, utc("2026-10-25T00:30:00Z"));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(
            parse_datetime("next tuesday", Europe::London)
                .unwrap_err()
                .kind(),
            "validation"
        );
        assert_eq!(parse_date("17/10/2026").unwrap_err().kind(), "validation");
        assert_eq!(
            parse_date("2026-10-17").unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()
        );
    }

    #[test]
    fn output_is_rfc3339_in_the_zone() {
        assert_eq!(
            format_in(utc("2026-07-01T08:00:00Z"), Europe::London),
            "2026-07-01T09:00:00+01:00"
        );
        assert_eq!(
            format_in(utc("2026-12-01T09:00:00Z"), Europe::London),
            "2026-12-01T09:00:00+00:00"
        );
    }
}
