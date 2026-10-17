//! New cards, and new content lines written exactly as given.

use super::model::ContactFields;
use super::parse::{GROUP_KIND, GROUP_MEMBER, joined_name};
use super::patch::{apply, touch};
use crate::error::{ContactsError, Result};
use chrono::{DateTime, Utc};
use mcp_icloud_dav::line::{Component, Property, escape_text};

pub const PRODID: &str = "-//Trozz//mcp-icloud//EN";

/// A property from its exact content-line text, such as
/// `TEL;type=CELL;type=VOICE;type=pref:+44 7700 900123`: the shared line layer keeps it as written
/// (repeated parameters, case, order) and folds it at 75 octets when the card is serialised.
pub fn property(line: &str) -> Result<Property> {
    Property::parse_line(line)
        .map_err(|_| ContactsError::Internal("built an invalid content line".into()))
}

/// A new contact card. It needs a given name, family name, display name or organization.
pub fn new_contact(uid: &str, fields: &ContactFields, now: DateTime<Utc>) -> Result<Component> {
    let given = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    if ![
        &fields.given_name,
        &fields.family_name,
        &fields.display_name,
        &fields.organization,
    ]
    .iter()
    .any(|v| given(v))
    {
        return Err(ContactsError::invalid(
            "a new contact needs given_name, family_name, display_name or organization",
        ));
    }
    let mut card = Component::new("VCARD");
    card.properties.push(Property::new("VERSION", "3.0"));
    card.properties.push(Property::new("PRODID", PRODID));
    card.properties.push(Property::new("N", ";;;;"));
    card.properties.push(Property::new("FN", ""));
    apply(&mut card, fields)?;
    let person = joined_name([
        &fields.prefix,
        &fields.given_name,
        &fields.middle_name,
        &fields.family_name,
        &fields.suffix,
    ]);
    if person.is_empty() && !given(&fields.display_name) && given(&fields.organization) {
        card.properties
            .push(Property::new_cased("X-ABShowAs", "COMPANY"));
    }
    touch(&mut card, now);
    card.properties.push(Property::new("UID", uid));
    Ok(card)
}

/// A new group card (an iOS List) listing the given contact UIDs.
pub fn new_group(uid: &str, name: &str, members: &[String], now: DateTime<Utc>) -> Component {
    let name = escape_text(name.trim());
    let mut card = Component::new("VCARD");
    card.properties.push(Property::new("VERSION", "3.0"));
    card.properties.push(Property::new("PRODID", PRODID));
    card.properties.push(Property::new("N", name.clone()));
    card.properties.push(Property::new("FN", name));
    card.properties.push(Property::new(GROUP_KIND, "group"));
    for m in members {
        card.properties
            .push(Property::new(GROUP_MEMBER, format!("urn:uuid:{m}")));
    }
    touch(&mut card, now);
    card.properties.push(Property::new("UID", uid));
    card
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::model::{Address, DateEntry, Entry, Related};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 17, 18, 30, 0).unwrap()
    }

    #[test]
    fn long_built_lines_are_folded_by_the_line_layer() {
        let p = property(&format!("NOTE:{}", "x".repeat(200))).unwrap();
        let mut card = Component::new("VCARD");
        card.properties.push(p);
        let text = card.to_ics();
        assert!(text.lines().all(|l| l.len() <= 75), "{text}");
        let back = Component::parse(&text).unwrap();
        assert_eq!(back.properties[0].value(), "x".repeat(200));
    }

    #[test]
    fn a_property_is_written_exactly_as_given() {
        let line = "TEL;type=CELL;type=VOICE;type=pref:+44 7700 900123";
        let mut card = Component::new("VCARD");
        card.properties.push(property(line).unwrap());
        assert_eq!(
            card.to_ics(),
            format!("BEGIN:VCARD\r\n{line}\r\nEND:VCARD\r\n")
        );
    }

    #[test]
    fn a_new_contact_has_every_given_field() {
        let fields = ContactFields {
            given_name: Some("Casey".into()),
            family_name: Some("Example".into()),
            organization: Some("Example Ltd".into()),
            job_title: Some("Engineer".into()),
            emails: Some(vec![Entry {
                value: "casey@example.com".into(),
                label: Some("work".into()),
                preferred: true,
            }]),
            phones: Some(vec![Entry {
                value: "+44 7700 900222".into(),
                label: Some("mobile".into()),
                preferred: false,
            }]),
            urls: Some(vec![Entry {
                value: "https://example.com".into(),
                label: Some("home page".into()),
                preferred: false,
            }]),
            addresses: Some(vec![Address {
                street: Some("2 Example Road".into()),
                city: Some("Exampleton".into()),
                postcode: Some("EX2 3CD".into()),
                label: Some("home".into()),
                ..Address::default()
            }]),
            dates: Some(vec![DateEntry {
                date: "2015-09-10".into(),
                label: Some("anniversary".into()),
            }]),
            related: Some(vec![Related {
                name: "Pat Example".into(),
                label: Some("sister".into()),
            }]),
            birthday: Some("--02-29".into()),
            note: Some("Line one\nline two, with a comma".into()),
            ..ContactFields::default()
        };
        let card = new_contact("NEW-UID-1", &fields, now()).unwrap();
        assert_eq!(
            card.to_ics(),
            [
                "BEGIN:VCARD",
                "VERSION:3.0",
                "PRODID:-//Trozz//mcp-icloud//EN",
                "N:Example;Casey;;;",
                "FN:Casey Example",
                "ORG:Example Ltd",
                "TITLE:Engineer",
                "NOTE:Line one\\nline two\\, with a comma",
                "BDAY;X-APPLE-OMIT-YEAR=1604:1604-02-29",
                "EMAIL;type=INTERNET;type=WORK;type=pref:casey@example.com",
                "TEL;type=CELL;type=VOICE:+44 7700 900222",
                "item1.URL:https://example.com",
                "item1.X-ABLabel:_$!<HomePage>!$_",
                "ADR;type=HOME:;;2 Example Road;Exampleton;;EX2 3CD;",
                "item2.X-ABDATE:2015-09-10",
                "item2.X-ABLabel:_$!<Anniversary>!$_",
                "item3.X-ABRELATEDNAMES:Pat Example",
                "item3.X-ABLabel:_$!<Sister>!$_",
                "REV:2026-10-17T18:30:00Z",
                "UID:NEW-UID-1",
                "END:VCARD",
                "",
            ]
            .join("\r\n")
        );
    }

    #[test]
    fn an_organization_only_contact_is_shown_as_a_company() {
        let fields = ContactFields {
            organization: Some("Example Bakery".into()),
            ..ContactFields::default()
        };
        let text = new_contact("C-1", &fields, now()).unwrap().to_ics();
        assert!(text.contains("\r\nFN:Example Bakery\r\n"), "{text}");
        assert!(text.contains("\r\nN:;;;;\r\n"), "{text}");
        assert!(text.contains("\r\nX-ABShowAs:COMPANY\r\n"), "{text}");
    }

    #[test]
    fn a_new_contact_needs_a_name_or_organization() {
        let fields = ContactFields {
            note: Some("just a note".into()),
            given_name: Some("  ".into()),
            ..ContactFields::default()
        };
        let e = new_contact("C-2", &fields, now()).unwrap_err();
        assert_eq!(e.kind(), "validation");
    }

    #[test]
    fn a_new_group_lists_its_members_by_uid() {
        let card = new_group(
            "G-1",
            "Book Club",
            &["A-1".to_string(), "B-2".to_string()],
            now(),
        );
        assert_eq!(
            card.to_ics(),
            [
                "BEGIN:VCARD",
                "VERSION:3.0",
                "PRODID:-//Trozz//mcp-icloud//EN",
                "N:Book Club",
                "FN:Book Club",
                "X-ADDRESSBOOKSERVER-KIND:group",
                "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:A-1",
                "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:B-2",
                "REV:2026-10-17T18:30:00Z",
                "UID:G-1",
                "END:VCARD",
                "",
            ]
            .join("\r\n")
        );
    }
}
