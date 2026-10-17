//! The shared content-line layer against the calendar's iCalendar fixtures.
use mcp_icloud_dav::line::{Component, unfold};

const APPLE: &str = include_str!("fixtures/apple-allday.ics");
const GOOGLE: &str = include_str!("fixtures/google-invite.ics");
const OUTLOOK: &str = include_str!("fixtures/outlook-invite.ics");
const WEEKLY: &str = include_str!("fixtures/apple-weekly.ics");
const OVERRIDE: &str = include_str!("fixtures/master-override.ics");

#[test]
fn unedited_resources_round_trip_exactly_after_unfolding() {
    for text in [APPLE, GOOGLE, OUTLOOK, WEEKLY, OVERRIDE] {
        let cal = Component::parse(text).unwrap();
        assert_eq!(unfold(&cal.to_ics()), unfold(text));
    }
}

#[test]
fn apple_unique_key_backslashes_survive() {
    let cal = Component::parse(APPLE).unwrap();
    let ev = cal.children("VEVENT").next().unwrap();
    let key = ev.value("X-APPLE-SUGGESTION-INFO-UNIQUE-KEY").unwrap();
    assert!(key.starts_with("|2|\\\\|booking-1@example.com|\\\\|5\\\\|\\\\\\\\\\\\|C2E1"));
    assert!(cal.to_ics().contains("|\\\\|5\\\\|"));
}

#[test]
fn editing_one_property_leaves_every_other_line_unchanged() {
    let mut cal = Component::parse(APPLE).unwrap();
    let ev = cal.children_mut("VEVENT").next().unwrap();
    ev.prop_mut("SUMMARY").unwrap().set_value("Renamed");
    let before = unfold(APPLE);
    let after = unfold(&cal.to_ics());
    assert_eq!(before.len(), after.len());
    let diff: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].1, "SUMMARY:Renamed");
}

#[test]
fn params_with_quotes_commas_and_colons_parse() {
    let cal = Component::parse(GOOGLE).unwrap();
    let ev = cal.children("VEVENT").next().unwrap();
    let me = ev
        .props("ATTENDEE")
        .find(|p| p.value() == "mailto:user@example.com")
        .unwrap();
    assert_eq!(me.param("CN"), Some("User, Example"));
    assert_eq!(me.param("partstat"), Some("NEEDS-ACTION"));
    assert_eq!(me.param("RSVP"), Some("TRUE"));
    let outlook = Component::parse(OUTLOOK).unwrap();
    let ev = outlook.children("VEVENT").next().unwrap();
    assert_eq!(
        ev.prop("DTSTART").unwrap().param("TZID"),
        Some("GMT Standard Time")
    );
}
