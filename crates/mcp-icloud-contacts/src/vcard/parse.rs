//! Reading cards: kind, UID, the contact view and the group view, from the lossless line tree.

use super::labels::{Field, read_label};
use super::model::{Address, Contact, DateEntry, Entry, Group, Related};
use crate::error::{ContactsError, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use mcp_icloud_dav::line::{Component, Property, unescape_text, unfold};
use std::collections::HashMap;

pub const GROUP_KIND: &str = "X-ADDRESSBOOKSERVER-KIND";
pub const GROUP_MEMBER: &str = "X-ADDRESSBOOKSERVER-MEMBER";

/// A card is a contact, or a group (an iOS List) when it says `X-ADDRESSBOOKSERVER-KIND:group`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Contact,
    Group,
}

/// Parses one stored card. Anything but a single VCARD is an error.
pub fn parse_card(text: &str) -> Result<Component> {
    let card = Component::parse(text)?;
    if !card.name.eq_ignore_ascii_case("VCARD") {
        return Err(ContactsError::Upstream(
            "the resource is not a vCard".into(),
        ));
    }
    Ok(card)
}

/// The UID of stored text, also when the text does not parse as a card.
pub fn uid_of_text(text: &str) -> Option<String> {
    unfold(text).into_iter().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        let name = name.split(';').next().unwrap_or(name);
        if !name.eq_ignore_ascii_case("UID") {
            return None;
        }
        let uid = unescape_text(value.trim());
        (!uid.is_empty()).then_some(uid)
    })
}

/// Properties with this name, with or without an `itemN.` group, with their index.
pub fn named<'a>(
    card: &'a Component,
    name: &'a str,
) -> impl Iterator<Item = (usize, &'a Property)> + 'a {
    card.properties
        .iter()
        .enumerate()
        .filter(move |(_, p)| p.base_name().eq_ignore_ascii_case(name))
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The first property with this name as text: unescaped, trimmed, never empty.
pub fn text(card: &Component, name: &str) -> Option<String> {
    named(card, name).find_map(|(_, p)| non_empty(&unescape_text(p.value())))
}

pub fn uid(card: &Component) -> Option<String> {
    text(card, "UID")
}

pub fn kind(card: &Component) -> Kind {
    match text(card, GROUP_KIND) {
        Some(k) if k.eq_ignore_ascii_case("group") => Kind::Group,
        _ => Kind::Contact,
    }
}

/// Splits a structured value (N, ORG, ADR) at unescaped `;` and unescapes each component.
pub fn components(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in value.chars() {
        if escaped {
            cur.push('\\');
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            ';' => out.push(unescape_text(&std::mem::take(&mut cur))),
            c => cur.push(c),
        }
    }
    if escaped {
        cur.push('\\');
    }
    out.push(unescape_text(&cur));
    out
}

/// Every `TYPE` value of a property, from repeated (`type=CELL;type=VOICE`) and listed
/// (`TYPE=CELL,VOICE`) parameters.
pub fn types(p: &Property) -> Vec<String> {
    p.params()
        .iter()
        .filter(|x| x.name.eq_ignore_ascii_case("TYPE"))
        .flat_map(|x| x.values.iter().flat_map(|v| v.split(',')))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn preferred(p: &Property) -> bool {
    types(p).iter().any(|t| t.eq_ignore_ascii_case("pref")) || p.param("PREF").is_some()
}

/// `X-ABLabel` values by lower-case `itemN` group.
fn group_labels(card: &Component) -> HashMap<String, String> {
    named(card, "X-ABLABEL")
        .filter_map(|(_, p)| Some((p.group()?.to_ascii_lowercase(), unescape_text(p.value()))))
        .collect()
}

fn label_of(field: Field, p: &Property, labels: &HashMap<String, String>) -> Option<String> {
    let ab = p
        .group()
        .and_then(|g| labels.get(&g.to_ascii_lowercase()))
        .map(String::as_str);
    read_label(field, &types(p), ab)
}

/// Emails, phones or URLs, with the index of their line.
pub fn entries(card: &Component, field: Field) -> Vec<(usize, Entry)> {
    let labels = group_labels(card);
    named(card, field.property())
        .filter_map(|(i, p)| {
            let value = non_empty(&unescape_text(p.value()))?;
            Some((
                i,
                Entry {
                    value,
                    label: label_of(field, p, &labels),
                    preferred: preferred(p),
                },
            ))
        })
        .collect()
}

/// Postal addresses, with the index of their line. The PO box, extended address and street
/// components are shown together as `street`, one per line.
pub fn addresses(card: &Component) -> Vec<(usize, Address)> {
    let labels = group_labels(card);
    named(card, "ADR")
        .filter_map(|(i, p)| {
            let mut c = components(p.value());
            c.resize(7, String::new());
            let street: Vec<&str> = c[..3]
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            let address = Address {
                street: non_empty(&street.join("\n")),
                city: non_empty(&c[3]),
                region: non_empty(&c[4]),
                postcode: non_empty(&c[5]),
                country: non_empty(&c[6]),
                label: label_of(Field::Address, p, &labels),
                preferred: preferred(p),
            };
            let empty = address.street.is_none()
                && address.city.is_none()
                && address.region.is_none()
                && address.postcode.is_none()
                && address.country.is_none();
            (!empty).then_some((i, address))
        })
        .collect()
}

fn omits_year(p: &Property) -> bool {
    p.param("X-APPLE-OMIT-YEAR").is_some()
}

/// Anniversaries and other dates (`X-ABDATE`), with the index of their line.
pub fn dates(card: &Component) -> Vec<(usize, DateEntry)> {
    let labels = group_labels(card);
    named(card, Field::Date.property())
        .filter_map(|(i, p)| {
            let date = non_empty(p.value())?;
            Some((
                i,
                DateEntry {
                    date: display_date(&date, omits_year(p)),
                    label: label_of(Field::Date, p, &labels),
                },
            ))
        })
        .collect()
}

/// Related names (`X-ABRELATEDNAMES`), with the index of their line.
pub fn related(card: &Component) -> Vec<(usize, Related)> {
    let labels = group_labels(card);
    named(card, Field::Related.property())
        .filter_map(|(i, p)| {
            let name = non_empty(&unescape_text(p.value()))?;
            Some((
                i,
                Related {
                    name,
                    label: label_of(Field::Related, p, &labels),
                },
            ))
        })
        .collect()
}

/// `1980-05-12`, `19800512`, a date-time or `--0512` as `YYYY-MM-DD`, or `--MM-DD` when the year
/// is omitted. Anything else is returned unchanged.
pub fn display_date(value: &str, omit_year: bool) -> String {
    let v = value.trim();
    let date = v.split('T').next().unwrap_or(v);
    let digits: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
    if date.starts_with("--") && digits.len() == 4 {
        return format!("--{}-{}", &digits[..2], &digits[2..]);
    }
    if !date.starts_with('-') && digits.len() == 8 {
        if omit_year {
            return format!("--{}-{}", &digits[4..6], &digits[6..]);
        }
        return format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..]);
    }
    v.to_string()
}

/// `REV` as RFC 3339 UTC (`2026-10-17T18:23:20Z`), also from the basic form `20261017T182320Z`.
pub fn display_rev(value: &str) -> String {
    let v = value.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(v) {
        return t
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
    }
    if let Ok(t) = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%SZ") {
        return t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    }
    v.to_string()
}

/// A member line's contact UID: `urn:uuid:<UID>` or a bare UID.
pub fn member_uid(value: &str) -> Option<String> {
    let v = unescape_text(value.trim());
    let uid = match v.get(..9) {
        Some(prefix) if prefix.eq_ignore_ascii_case("urn:uuid:") => v[9..].trim().to_string(),
        _ => v,
    };
    (!uid.is_empty()).then_some(uid)
}

/// The name parts joined like iOS shows them.
pub fn joined_name(parts: [&Option<String>; 5]) -> String {
    parts
        .iter()
        .filter_map(|p| p.as_deref())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_contact(card: &Component, id: &str) -> Contact {
    let labels = group_labels(card);
    let mut c = Contact {
        id: id.to_string(),
        ..Contact::default()
    };
    if let Some((_, n)) = named(card, "N").next() {
        let mut parts = components(n.value());
        parts.resize(5, String::new());
        c.family_name = non_empty(&parts[0]);
        c.given_name = non_empty(&parts[1]);
        c.middle_name = non_empty(&parts[2]);
        c.prefix = non_empty(&parts[3]);
        c.suffix = non_empty(&parts[4]);
    }
    if let Some((_, org)) = named(card, "ORG").next() {
        let parts = components(org.value());
        c.organization = parts.first().and_then(|o| non_empty(o));
        let units: Vec<&str> = parts
            .iter()
            .skip(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        c.department = non_empty(&units.join(", "));
    }
    c.nickname = text(card, "NICKNAME");
    c.job_title = text(card, "TITLE");
    c.note = text(card, "NOTE");
    c.emails = entries(card, Field::Email)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    c.phones = entries(card, Field::Phone)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    c.urls = entries(card, Field::Url)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    c.addresses = addresses(card).into_iter().map(|(_, a)| a).collect();
    c.dates = dates(card).into_iter().map(|(_, d)| d).collect();
    c.related = related(card).into_iter().map(|(_, r)| r).collect();
    c.birthday = named(card, "BDAY")
        .find_map(|(_, p)| non_empty(p.value()).map(|v| display_date(&v, omits_year(p))));
    c.updated = named(card, "REV").find_map(|(_, p)| non_empty(p.value()).map(|v| display_rev(&v)));
    c.has_photo = named(card, "PHOTO").next().is_some();
    c.im = named(card, "IMPP")
        .filter_map(|(_, p)| {
            let value = non_empty(&unescape_text(p.value()))?;
            let label = label_of(Field::Email, p, &labels)
                .or_else(|| p.param("X-SERVICE-TYPE").and_then(non_empty))
                .map(|l| l.to_lowercase());
            Some(Entry {
                value,
                label,
                preferred: preferred(p),
            })
        })
        .collect();
    c.social = named(card, "X-SOCIALPROFILE")
        .filter_map(|(_, p)| {
            let value = non_empty(&unescape_text(p.value()))?;
            Some(Entry {
                value,
                label: types(p).first().map(|t| t.to_lowercase()),
                preferred: false,
            })
        })
        .collect();
    let name = joined_name([
        &c.prefix,
        &c.given_name,
        &c.middle_name,
        &c.family_name,
        &c.suffix,
    ]);
    c.display_name = text(card, "FN")
        .or_else(|| non_empty(&name))
        .or_else(|| c.organization.clone())
        .or_else(|| c.emails.first().map(|e| e.value.clone()))
        .or_else(|| c.phones.first().map(|e| e.value.clone()))
        .unwrap_or_default();
    c
}

pub fn parse_group(card: &Component, id: &str) -> Group {
    let name = text(card, "FN")
        .or_else(|| named(card, "N").find_map(|(_, p)| non_empty(&components(p.value())[0])))
        .unwrap_or_default();
    let members = named(card, GROUP_MEMBER)
        .filter_map(|(_, p)| member_uid(p.value()))
        .collect();
    Group {
        id: id.to_string(),
        name,
        members,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERSON: &str = include_str!("../../tests/fixtures/apple-person.vcf");
    const COMPANY: &str = include_str!("../../tests/fixtures/apple-company.vcf");
    const GROUP: &str = include_str!("../../tests/fixtures/apple-group.vcf");
    const OTHER: &str = include_str!("../../tests/fixtures/other-client.vcf");

    /// The fixtures are LF text; iCloud stores CRLF.
    fn icloud(text: &str) -> String {
        text.replace('\n', "\r\n")
    }

    fn entry(value: &str, label: Option<&str>, preferred: bool) -> Entry {
        Entry {
            value: value.into(),
            label: label.map(str::to_string),
            preferred,
        }
    }

    #[test]
    fn apple_cards_round_trip_byte_for_byte() {
        for text in [PERSON, COMPANY, GROUP] {
            let text = icloud(text);
            assert_eq!(parse_card(&text).unwrap().to_ics(), text);
        }
    }

    #[test]
    fn an_apple_contact_shows_every_field() {
        let card = parse_card(&icloud(PERSON)).unwrap();
        assert_eq!(kind(&card), Kind::Contact);
        assert_eq!(
            uid(&card).as_deref(),
            Some("5F1C2D3E-4A5B-4C6D-8E7F-901234567890")
        );
        let c = parse_contact(&card, "5F1C2D3E-4A5B-4C6D-8E7F-901234567890");
        assert_eq!(c.display_name, "Dr. Alex J. Example Jr.");
        assert_eq!(c.given_name.as_deref(), Some("Alex"));
        assert_eq!(c.family_name.as_deref(), Some("Example"));
        assert_eq!(c.middle_name.as_deref(), Some("J."));
        assert_eq!(c.prefix.as_deref(), Some("Dr."));
        assert_eq!(c.suffix.as_deref(), Some("Jr."));
        assert_eq!(c.nickname.as_deref(), Some("Lex"));
        assert_eq!(c.organization.as_deref(), Some("Example Widgets Ltd"));
        assert_eq!(c.department.as_deref(), Some("Research"));
        assert_eq!(c.job_title.as_deref(), Some("Chief Tinkerer"));
        assert_eq!(
            c.emails,
            [
                entry("alex@example.com", Some("home"), true),
                entry("alex.work@example.org", Some("office"), false),
            ]
        );
        assert_eq!(
            c.phones,
            [
                entry("+44 7700 900123", Some("mobile"), true),
                entry("07700 900456", Some("home"), false),
                entry("+1 202-555-0123", Some("boat"), false),
            ]
        );
        assert_eq!(
            c.addresses,
            [Address {
                street: Some("1 Example Street\nFlat 2".into()),
                city: Some("Exampleton".into()),
                region: Some("Example County".into()),
                postcode: Some("EX1 2AB".into()),
                country: Some("United Kingdom".into()),
                label: Some("home".into()),
                preferred: true,
            }]
        );
        assert_eq!(
            c.urls,
            [entry("https://example.com/alex", Some("home page"), true)]
        );
        assert_eq!(c.birthday.as_deref(), Some("--05-12"));
        assert_eq!(
            c.dates,
            [DateEntry {
                date: "2010-06-01".into(),
                label: Some("anniversary".into())
            }]
        );
        assert_eq!(
            c.related,
            [
                Related {
                    name: "Sam Example".into(),
                    label: Some("mother".into())
                },
                Related {
                    name: "Robin Example".into(),
                    label: Some("best friend".into())
                },
            ]
        );
        assert_eq!(c.im, [entry("skype:alex.example", Some("skype"), true)]);
        assert_eq!(
            c.social,
            [entry(
                "https://twitter.com/alexexample",
                Some("twitter"),
                false
            )]
        );
        assert_eq!(
            c.note.as_deref(),
            Some("Met at the conference, 2019.\nLikes tea.")
        );
        assert!(c.has_photo);
        assert_eq!(c.updated.as_deref(), Some("2026-10-17T18:23:20Z"));
        assert!(!c.is_me);
        assert!(c.groups.is_empty());
    }

    #[test]
    fn a_company_card_shows_its_organization() {
        let card = parse_card(&icloud(COMPANY)).unwrap();
        let c = parse_contact(&card, "x");
        assert_eq!(c.display_name, "Example Plumbing Co");
        assert_eq!(c.organization.as_deref(), Some("Example Plumbing Co"));
        assert_eq!(c.department, None);
        assert_eq!(c.given_name, None);
        assert_eq!(c.phones, [entry("+44 7700 900789", Some("work"), true)]);
        assert_eq!(
            c.urls,
            [entry("https://example.net", Some("home page"), false)]
        );
        assert!(!c.has_photo);
    }

    #[test]
    fn a_group_card_lists_member_uids_in_both_forms() {
        let card = parse_card(&icloud(GROUP)).unwrap();
        assert_eq!(kind(&card), Kind::Group);
        let g = parse_group(&card, "9B8A7C6D-1111-4222-8333-444455556666");
        assert_eq!(g.name, "Book Club");
        assert_eq!(
            g.members,
            [
                "5F1C2D3E-4A5B-4C6D-8E7F-901234567890",
                "0A1B2C3D-0000-4000-8000-00000000C0DE",
                "DEADBEEF-0000-4000-8000-000000000000",
            ]
        );
    }

    #[test]
    fn cards_from_other_clients_read_the_same_way() {
        let card = parse_card(OTHER).unwrap();
        let c = parse_contact(&card, "other-client-0001");
        assert_eq!(c.display_name, "Jordan Sample");
        assert_eq!(c.phones, [entry("07700 900321", Some("mobile"), false)]);
        assert_eq!(c.emails, [entry("jordan@example.net", Some("work"), false)]);
        assert_eq!(c.birthday.as_deref(), Some("1985-03-02"));
        assert_eq!(c.updated.as_deref(), Some("2025-01-02T03:04:05Z"));
    }

    #[test]
    fn group_kind_is_case_insensitive() {
        let card =
            parse_card("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:G\r\nx-addressbookserver-kind:Group\r\nUID:g\r\nEND:VCARD\r\n")
                .unwrap();
        assert_eq!(kind(&card), Kind::Group);
    }

    #[test]
    fn structured_values_split_at_unescaped_semicolons() {
        assert_eq!(components(r"a\;b;c\,d;"), ["a;b", "c,d", ""]);
        assert_eq!(components("only"), ["only"]);
    }

    #[test]
    fn dates_read_in_every_accepted_form() {
        assert_eq!(display_date("1980-05-12", false), "1980-05-12");
        assert_eq!(display_date("19800512", false), "1980-05-12");
        assert_eq!(display_date("1980-05-12T00:00:00Z", false), "1980-05-12");
        assert_eq!(display_date("--0512", false), "--05-12");
        assert_eq!(display_date("--05-12", false), "--05-12");
        assert_eq!(display_date("1604-05-12", true), "--05-12");
        assert_eq!(display_date("sometime", false), "sometime");
    }

    #[test]
    fn member_uids_drop_the_urn_prefix() {
        assert_eq!(member_uid("urn:uuid:ABC").as_deref(), Some("ABC"));
        assert_eq!(member_uid("URN:UUID:ABC").as_deref(), Some("ABC"));
        assert_eq!(member_uid("ABC").as_deref(), Some("ABC"));
        assert_eq!(member_uid("  "), None);
    }

    #[test]
    fn the_uid_is_found_even_when_the_card_does_not_parse() {
        let broken = "BEGIN:VCARD\r\nFN:Broken\r\nUID:BROKEN-1\r\n";
        assert!(parse_card(broken).is_err());
        assert_eq!(uid_of_text(broken).as_deref(), Some("BROKEN-1"));
        assert_eq!(uid_of_text("BEGIN:VCARD\r\nFN:x\r\n"), None);
    }

    #[test]
    fn a_calendar_is_not_a_card() {
        assert!(parse_card("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n").is_err());
    }

    #[test]
    fn the_display_name_falls_back_in_order() {
        let c = |body: &str| {
            let text = format!("BEGIN:VCARD\r\nVERSION:3.0\r\n{body}UID:u\r\nEND:VCARD\r\n");
            parse_contact(&parse_card(&text).unwrap(), "u").display_name
        };
        assert_eq!(c("FN:Shown\r\nN:Family;Given;;;\r\n"), "Shown");
        assert_eq!(c("FN:\r\nN:Family;Given;;Ms;\r\n"), "Ms Given Family");
        assert_eq!(c("ORG:Example Org;\r\n"), "Example Org");
        assert_eq!(c("EMAIL:someone@example.com\r\n"), "someone@example.com");
        assert_eq!(c("TEL:07700 900111\r\n"), "07700 900111");
        assert_eq!(c(""), "");
    }
}
