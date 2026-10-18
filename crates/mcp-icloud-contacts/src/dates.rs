//! The next birthdays, anniversaries and other dates of the indexed contacts.

use crate::search::normalize;
use crate::vcard::model::Contact;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// Days ahead `upcoming_dates` looks without a `days` argument.
pub const DEFAULT_DAYS: u32 = 30;
/// The largest `days` argument.
pub const MAX_DAYS: u32 = 366;

/// One coming date of one contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Upcoming {
    pub id: String,
    pub display_name: String,
    /// `birthday`, the date's own label (such as `anniversary`), or `date` without one.
    pub label: String,
    /// The next occurrence, `YYYY-MM-DD`.
    pub date: String,
    /// Days from today (0 is today).
    pub in_days: i64,
    /// Age or number of years reached on `date`, when the stored date has a year.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub years: Option<i32>,
    pub is_me: bool,
}

/// Year (when known), month and day of a stored `YYYY-MM-DD` or `--MM-DD` date. None for
/// anything else, including a month and day that never exist.
pub fn parse_stored(value: &str) -> Option<(Option<i32>, u32, u32)> {
    let value = value.trim();
    if let Some(month_day) = value.strip_prefix("--") {
        let (m, d) = month_day.split_once('-')?;
        let (month, day) = (m.parse().ok()?, d.parse().ok()?);
        NaiveDate::from_ymd_opt(2000, month, day)?;
        return Some((None, month, day));
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    Some((Some(date.year()), date.month(), date.day()))
}

/// The month and day in `year`; 29 February falls on 28 February in other years.
fn in_year(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day).or_else(|| {
        if month == 2 && day == 29 {
            return NaiveDate::from_ymd_opt(year, 2, 28);
        }
        None
    })
}

/// The first date on or after `today` with this month and day.
pub fn next_occurrence(month: u32, day: u32, today: NaiveDate) -> Option<NaiveDate> {
    let this_year = in_year(today.year(), month, day)?;
    if this_year >= today {
        return Some(this_year);
    }
    in_year(today.year() + 1, month, day)
}

fn entry(c: &Contact, label: &str, stored: &str, today: NaiveDate, days: u32) -> Option<Upcoming> {
    let (year, month, day) = parse_stored(stored)?;
    let date = next_occurrence(month, day, today)?;
    let in_days = (date - today).num_days();
    if in_days > i64::from(days) {
        return None;
    }
    Some(Upcoming {
        id: c.id.clone(),
        display_name: c.display_name.clone(),
        label: label.to_string(),
        date: date.format("%Y-%m-%d").to_string(),
        in_days,
        years: year.map(|y| date.year() - y).filter(|n| *n >= 0),
        is_me: c.is_me,
    })
}

/// Birthdays and dates from today up to `days` days ahead (both ends included), soonest first,
/// then by display name and id.
pub fn upcoming(contacts: &[Contact], today: NaiveDate, days: u32) -> Vec<Upcoming> {
    let mut found: Vec<Upcoming> = Vec::new();
    for c in contacts {
        if let Some(b) = &c.birthday {
            found.extend(entry(c, "birthday", b, today, days));
        }
        for d in &c.dates {
            let label = d.label.as_deref().unwrap_or("date");
            found.extend(entry(c, label, &d.date, today, days));
        }
    }
    found.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| normalize(&a.display_name).cmp(&normalize(&b.display_name)))
            .then_with(|| a.id.cmp(&b.id))
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::model::DateEntry;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn person(
        id: &str,
        name: &str,
        birthday: Option<&str>,
        dates: &[(&str, Option<&str>)],
    ) -> Contact {
        Contact {
            id: id.into(),
            display_name: name.into(),
            birthday: birthday.map(str::to_string),
            dates: dates
                .iter()
                .map(|(date, label)| DateEntry {
                    date: date.to_string(),
                    label: label.map(str::to_string),
                })
                .collect(),
            ..Contact::default()
        }
    }

    #[test]
    fn stored_dates_with_and_without_a_year() {
        assert_eq!(parse_stored("1985-03-02"), Some((Some(1985), 3, 2)));
        assert_eq!(parse_stored("--05-12"), Some((None, 5, 12)));
        assert_eq!(parse_stored(" --02-29 "), Some((None, 2, 29)));
        for bad in [
            "",
            "--13-01",
            "--02-30",
            "1985-02-30",
            "19850302",
            "soon",
            "--5",
        ] {
            assert_eq!(parse_stored(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_next_occurrence_is_today_later_this_year_or_next_year() {
        let today = day("2026-10-18");
        assert_eq!(next_occurrence(10, 18, today), Some(day("2026-10-18")));
        assert_eq!(next_occurrence(10, 19, today), Some(day("2026-10-19")));
        assert_eq!(next_occurrence(10, 17, today), Some(day("2027-10-17")));
        assert_eq!(next_occurrence(1, 1, today), Some(day("2027-01-01")));
        assert_eq!(
            next_occurrence(12, 31, day("2026-12-31")),
            Some(day("2026-12-31"))
        );
    }

    #[test]
    fn the_29th_of_february_falls_on_the_28th_in_other_years() {
        assert_eq!(
            next_occurrence(2, 29, day("2026-02-01")),
            Some(day("2026-02-28"))
        );
        assert_eq!(
            next_occurrence(2, 29, day("2026-02-28")),
            Some(day("2026-02-28"))
        );
        assert_eq!(
            next_occurrence(2, 29, day("2026-03-01")),
            Some(day("2027-02-28"))
        );
        assert_eq!(
            next_occurrence(2, 29, day("2027-03-01")),
            Some(day("2028-02-29"))
        );
    }

    #[test]
    fn birthdays_and_labelled_dates_are_listed_soonest_first() {
        let contacts = [
            person(
                "b",
                "Bea Example",
                Some("1990-10-20"),
                &[("2015-11-01", Some("anniversary"))],
            ),
            person(
                "a",
                "Alex Example",
                Some("--10-20"),
                &[("2020-10-25", None)],
            ),
            person("c", "Cal Example", Some("1980-01-05"), &[]),
        ];
        let found = upcoming(&contacts, day("2026-10-18"), 30);
        let rows: Vec<(&str, &str, &str, i64, Option<i32>)> = found
            .iter()
            .map(|u| {
                (
                    u.id.as_str(),
                    u.label.as_str(),
                    u.date.as_str(),
                    u.in_days,
                    u.years,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("a", "birthday", "2026-10-20", 2, None),
                ("b", "birthday", "2026-10-20", 2, Some(36)),
                ("a", "date", "2026-10-25", 7, Some(6)),
                ("b", "anniversary", "2026-11-01", 14, Some(11)),
            ]
        );
    }

    #[test]
    fn days_counts_today_and_the_last_day() {
        let contacts = [
            person("t", "Today", Some("--10-18"), &[]),
            person("l", "Last", Some("--10-19"), &[]),
            person("x", "Beyond", Some("--10-20"), &[]),
        ];
        let found = upcoming(&contacts, day("2026-10-18"), 1);
        let ids: Vec<&str> = found.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["t", "l"]);
        assert_eq!(found[0].in_days, 0);
    }

    #[test]
    fn a_whole_year_ahead_finds_every_date_once() {
        let contacts = [person(
            "p",
            "Pat",
            Some("2001-10-17"),
            &[("--01-01", Some("party"))],
        )];
        let found = upcoming(&contacts, day("2026-10-18"), MAX_DAYS);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].date, "2027-01-01");
        assert_eq!(found[1].date, "2027-10-17");
        assert_eq!(found[1].years, Some(26));
        assert_eq!(found[1].in_days, 364);
    }

    #[test]
    fn a_stored_year_after_the_occurrence_has_no_years() {
        let contacts = [person("f", "Future", Some("2030-10-20"), &[])];
        let found = upcoming(&contacts, day("2026-10-18"), 30);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].years, None);
    }

    #[test]
    fn unreadable_dates_and_contacts_without_dates_are_skipped() {
        let contacts = [
            person(
                "u",
                "Unreadable",
                Some("sometime"),
                &[("--02-30", Some("odd"))],
            ),
            person("n", "No dates", None, &[]),
        ];
        assert!(upcoming(&contacts, day("2026-10-18"), MAX_DAYS).is_empty());
    }

    #[test]
    fn upcoming_entries_serialise_without_years_when_unknown() {
        let found = upcoming(
            &[person("a", "Alex", Some("--10-20"), &[])],
            day("2026-10-18"),
            30,
        );
        assert_eq!(
            serde_json::to_value(&found[0]).unwrap(),
            serde_json::json!({
                "id": "a", "display_name": "Alex", "label": "birthday",
                "date": "2026-10-20", "in_days": 2, "is_me": false
            })
        );
    }
}
