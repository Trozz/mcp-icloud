use crate::error::{CalError, Result};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use mcp_icloud_dav::line::Property;

const DATE_FORMAT: &str = "%Y%m%d";
const LOCAL_FORMAT: &str = "%Y%m%dT%H%M%S";
const UTC_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// A DATE or DATE-TIME value as iCalendar stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    Date(NaiveDate),
    Utc(DateTime<Utc>),
    Zoned { local: NaiveDateTime, tzid: String },
    Floating(NaiveDateTime),
}

impl When {
    pub fn is_date(&self) -> bool {
        matches!(self, When::Date(_))
    }

    pub fn tzid(&self) -> Option<&str> {
        match self {
            When::Zoned { tzid, .. } => Some(tzid),
            _ => None,
        }
    }

    /// The zone this value is read in: its TZID when it is an IANA name, else `fallback`.
    pub fn zone(&self, fallback: Tz) -> Tz {
        match self {
            When::Zoned { tzid, .. } => tzid.parse().unwrap_or(fallback),
            When::Utc(_) => Tz::UTC,
            _ => fallback,
        }
    }

    /// The absolute instant. Dates are midnight in `fallback`; floating times and TZIDs that are
    /// not IANA names are read in `fallback`. Stored times inside a DST gap move forward one hour,
    /// as calendar clients do.
    pub fn instant(&self, fallback: Tz) -> DateTime<Utc> {
        match self {
            When::Utc(dt) => *dt,
            When::Date(d) => lenient_local(d.and_hms_opt(0, 0, 0).unwrap_or_default(), fallback),
            When::Zoned { local, .. } => lenient_local(*local, self.zone(fallback)),
            When::Floating(local) => lenient_local(*local, fallback),
        }
    }

    /// Adds a duration. Dates move by whole days. None when the result leaves years 0 to 9999.
    pub fn add(&self, d: Duration) -> Option<When> {
        let sum = match self {
            When::Date(date) => When::Date(date.checked_add_signed(Duration::days(d.num_days()))?),
            When::Utc(dt) => When::Utc(dt.checked_add_signed(d)?),
            When::Zoned { local, tzid } => When::Zoned {
                local: local.checked_add_signed(d)?,
                tzid: tzid.clone(),
            },
            When::Floating(local) => When::Floating(local.checked_add_signed(d)?),
        };
        sum.in_supported_years().then_some(sum)
    }

    fn in_supported_years(&self) -> bool {
        let year = match self {
            When::Date(d) => d.year(),
            When::Utc(dt) => dt.year(),
            When::Zoned { local, .. } | When::Floating(local) => local.year(),
        };
        (0..=9999).contains(&year)
    }

    /// The value part as written in iCalendar.
    pub fn value_string(&self) -> String {
        match self {
            When::Date(d) => d.format(DATE_FORMAT).to_string(),
            When::Utc(dt) => dt.format(UTC_FORMAT).to_string(),
            When::Zoned { local, .. } | When::Floating(local) => {
                local.format(LOCAL_FORMAT).to_string()
            }
        }
    }

    pub fn to_property(&self, name: &str) -> Property {
        let prop = Property::new(name, self.value_string());
        match self {
            When::Date(_) => prop.with_param("VALUE", "DATE"),
            When::Zoned { tzid, .. } => prop.with_param("TZID", tzid.clone()),
            _ => prop,
        }
    }

    pub fn from_property(p: &Property) -> Result<When> {
        let mut all = when_list(p)?;
        if all.is_empty() {
            return Err(bad_value(p));
        }
        Ok(all.swap_remove(0))
    }

    /// The same moment written in the style of `like` (DATE, UTC, TZID or floating), used to
    /// add EXDATE and RECURRENCE-ID values that match a series' DTSTART.
    pub fn in_style_of(instant: DateTime<Utc>, like: &When, fallback: Tz) -> When {
        match like {
            When::Date(_) => When::Date(instant.with_timezone(&fallback).date_naive()),
            When::Utc(_) => When::Utc(instant),
            When::Zoned { tzid, .. } => When::Zoned {
                local: instant.with_timezone(&like.zone(fallback)).naive_local(),
                tzid: tzid.clone(),
            },
            When::Floating(_) => When::Floating(instant.with_timezone(&fallback).naive_local()),
        }
    }
}

fn lenient_local(naive: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        LocalResult::None => match naive.checked_add_signed(Duration::hours(1)) {
            Some(next) => lenient_local(next, tz),
            None => naive.and_utc(),
        },
    }
}

fn bad_value(p: &Property) -> CalError {
    CalError::Upstream(format!("invalid {} value {:?}", p.name(), p.value()))
}

/// All values of a property that may hold a comma-separated list (EXDATE, RDATE).
pub fn when_list(p: &Property) -> Result<Vec<When>> {
    let is_date = p
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    let tzid = p.param("TZID");
    let mut out = Vec::new();
    for raw in p
        .value()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let when = if is_date || raw.len() == 8 {
            When::Date(NaiveDate::parse_from_str(raw, DATE_FORMAT).map_err(|_| bad_value(p))?)
        } else if let Some(utc) = raw.strip_suffix('Z') {
            let local =
                NaiveDateTime::parse_from_str(utc, LOCAL_FORMAT).map_err(|_| bad_value(p))?;
            When::Utc(local.and_utc())
        } else {
            let local =
                NaiveDateTime::parse_from_str(raw, LOCAL_FORMAT).map_err(|_| bad_value(p))?;
            match tzid {
                Some(t) => When::Zoned {
                    local,
                    tzid: t.to_string(),
                },
                None => When::Floating(local),
            }
        };
        out.push(when);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartStat {
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
    Delegated,
    Other(String),
}

impl PartStat {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "NEEDS-ACTION" => PartStat::NeedsAction,
            "ACCEPTED" => PartStat::Accepted,
            "DECLINED" => PartStat::Declined,
            "TENTATIVE" => PartStat::Tentative,
            "DELEGATED" => PartStat::Delegated,
            other => PartStat::Other(other.to_string()),
        }
    }

    /// The iCalendar value, e.g. `NEEDS-ACTION`.
    pub fn as_ical(&self) -> &str {
        match self {
            PartStat::NeedsAction => "NEEDS-ACTION",
            PartStat::Accepted => "ACCEPTED",
            PartStat::Declined => "DECLINED",
            PartStat::Tentative => "TENTATIVE",
            PartStat::Delegated => "DELEGATED",
            PartStat::Other(s) => s,
        }
    }

    /// Lower-case name for tool output, e.g. `needs_action`.
    pub fn as_output(&self) -> String {
        self.as_ical().to_ascii_lowercase().replace('-', "_")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attendee {
    pub email: String,
    pub name: Option<String>,
    pub status: PartStat,
    pub role: Option<String>,
    pub is_me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organizer {
    pub email: String,
    pub name: Option<String>,
    pub is_me: bool,
}

/// A read view of one VEVENT (a series master, an override or a single event).
#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub uid: String,
    pub recurrence_id: Option<When>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub url: Option<String>,
    pub start: When,
    /// Effective end: DTEND, else DTSTART plus DURATION, else the next day for dates, else DTSTART.
    pub end: When,
    pub rrule: Option<String>,
    pub exdates: Vec<When>,
    pub rdates: Vec<When>,
    pub attendees: Vec<Attendee>,
    pub organizer: Option<Organizer>,
    pub status: Option<String>,
    pub transparent: bool,
    pub alerts_minutes_before: Vec<i64>,
    pub sequence: u32,
}

impl EventData {
    pub fn my_status(&self) -> Option<&PartStat> {
        self.attendees.iter().find(|a| a.is_me).map(|a| &a.status)
    }

    pub fn i_organize(&self) -> bool {
        self.organizer.as_ref().is_some_and(|o| o.is_me)
    }

    /// True when someone other than me is invited.
    pub fn has_other_attendees(&self) -> bool {
        self.attendees.iter().any(|a| !a.is_me)
    }

    pub fn is_cancelled(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("CANCELLED"))
    }

    pub fn is_recurring(&self) -> bool {
        self.rrule.is_some() || !self.rdates.is_empty()
    }

    pub fn duration(&self, fallback: Tz) -> Duration {
        self.end.instant(fallback) - self.start.instant(fallback)
    }
}

/// My calendar user addresses (`calendar-user-address-set`), compared case-insensitively.
#[derive(Debug, Clone, Default)]
pub struct MyAddresses {
    addrs: Vec<String>,
}

impl MyAddresses {
    pub fn new<I, S>(addrs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            addrs: addrs
                .into_iter()
                .map(|a| normalize_address(a.as_ref()))
                .collect(),
        }
    }

    pub fn matches(&self, value: &str) -> bool {
        let v = normalize_address(value);
        !v.is_empty() && self.addrs.contains(&v)
    }
}

/// Lower-cases and strips a `mailto:` prefix and a trailing slash.
pub fn normalize_address(value: &str) -> String {
    let v = value.trim();
    let v = match v.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("mailto:") => &v[7..],
        _ => v,
    };
    v.trim_end_matches('/').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe;

    fn prop(line: &str) -> Property {
        let cal =
            mcp_icloud_dav::line::Component::parse(&format!("BEGIN:VEVENT\n{line}\nEND:VEVENT\n"))
                .unwrap();
        cal.properties[0].clone()
    }

    #[test]
    fn values_parse_into_the_four_kinds() {
        assert_eq!(
            When::from_property(&prop("DTSTART;VALUE=DATE:20260113")).unwrap(),
            When::Date(NaiveDate::from_ymd_opt(2026, 1, 13).unwrap())
        );
        let utc = When::from_property(&prop("DTSTART:20261020T130000Z")).unwrap();
        assert_eq!(
            utc.instant(Europe::London).to_rfc3339(),
            "2026-10-20T13:00:00+00:00"
        );
        let zoned =
            When::from_property(&prop("DTSTART;TZID=Europe/London:20260701T090000")).unwrap();
        assert_eq!(zoned.tzid(), Some("Europe/London"));
        assert_eq!(
            zoned.instant(Tz::UTC).to_rfc3339(),
            "2026-07-01T08:00:00+00:00"
        );
        let floating = When::from_property(&prop("DTSTART:20260701T090000")).unwrap();
        assert!(matches!(floating, When::Floating(_)));
        assert_eq!(
            floating.instant(Europe::London).to_rfc3339(),
            "2026-07-01T08:00:00+00:00"
        );
    }

    #[test]
    fn unknown_tzid_falls_back_to_the_given_zone() {
        let w =
            When::from_property(&prop("DTSTART;TZID=GMT Standard Time:20260701T100000")).unwrap();
        assert_eq!(w.zone(Europe::London), Europe::London);
        assert_eq!(
            w.instant(Europe::London).to_rfc3339(),
            "2026-07-01T09:00:00+00:00"
        );
    }

    #[test]
    fn stored_time_in_a_gap_moves_forward() {
        let w = When::Zoned {
            local: NaiveDate::from_ymd_opt(2026, 3, 29)
                .unwrap()
                .and_hms_opt(1, 30, 0)
                .unwrap(),
            tzid: "Europe/London".into(),
        };
        assert_eq!(w.instant(Tz::UTC).to_rfc3339(), "2026-03-29T01:30:00+00:00");
    }

    #[test]
    fn lists_and_bad_values() {
        let p = prop("EXDATE;TZID=Europe/London:20260119T100000,20260202T100000");
        assert_eq!(when_list(&p).unwrap().len(), 2);
        let e = When::from_property(&prop("DTSTART:yesterday")).unwrap_err();
        assert_eq!(e.kind(), "upstream");
    }

    #[test]
    fn properties_are_written_in_their_style() {
        let d = When::Date(NaiveDate::from_ymd_opt(2026, 12, 27).unwrap());
        assert_eq!(
            d.to_property("DTEND").to_line(),
            "DTEND;VALUE=DATE:20261227"
        );
        let z = When::from_property(&prop("DTSTART;TZID=Europe/London:20260701T090000")).unwrap();
        assert_eq!(
            z.add(Duration::minutes(30))
                .unwrap()
                .to_property("DTEND")
                .to_line(),
            "DTEND;TZID=Europe/London:20260701T093000"
        );
        let instant = z.instant(Tz::UTC);
        assert_eq!(When::in_style_of(instant, &z, Tz::UTC), z);
        assert_eq!(
            When::in_style_of(instant, &When::Utc(instant), Tz::UTC).value_string(),
            "20260701T080000Z"
        );
        assert_eq!(d.add(Duration::days(2)).unwrap().value_string(), "20261229");
    }

    #[test]
    fn add_out_of_range_is_none() {
        let far = When::Utc(DateTime::<Utc>::MAX_UTC);
        assert!(far.add(Duration::days(1)).is_none());
        let date = When::Date(NaiveDate::from_ymd_opt(9999, 12, 31).unwrap());
        assert!(date.add(Duration::days(1)).is_none());
        let local = When::Floating(
            NaiveDate::from_ymd_opt(9999, 12, 31)
                .unwrap()
                .and_hms_opt(23, 0, 0)
                .unwrap(),
        );
        assert!(local.add(Duration::hours(2)).is_none());
        assert!(local.add(Duration::hours(1) - Duration::hours(1)).is_some());
    }

    #[test]
    fn normalize_address_survives_multibyte_prefix() {
        assert_eq!(normalize_address("ééééé@x"), "ééééé@x");
        assert_eq!(normalize_address("MAILTO:A@x"), "a@x");
    }

    #[test]
    fn partstat_round_trips() {
        for s in [
            "NEEDS-ACTION",
            "ACCEPTED",
            "DECLINED",
            "TENTATIVE",
            "DELEGATED",
            "X-FOO",
        ] {
            assert_eq!(PartStat::parse(s).as_ical(), s);
        }
        assert_eq!(PartStat::parse("needs-action"), PartStat::NeedsAction);
        assert_eq!(PartStat::NeedsAction.as_output(), "needs_action");
    }

    #[test]
    fn my_addresses_match_mailto_paths_and_urns() {
        let me = MyAddresses::new([
            "mailto:User@Example.com",
            "/1234567890/principal/",
            "urn:uuid:1234567890",
        ]);
        assert!(me.matches("mailto:user@example.com"));
        assert!(me.matches("USER@EXAMPLE.COM"));
        assert!(me.matches("/1234567890/principal"));
        assert!(me.matches("urn:uuid:1234567890"));
        assert!(!me.matches("mailto:someone@example.org"));
        assert!(!me.matches(""));
    }
}
