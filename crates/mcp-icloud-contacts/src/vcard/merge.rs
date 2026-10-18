//! What merging contacts adds to the contact that is kept, as `update_contact` fields: applying
//! them with `patch::apply` keeps every existing line of the kept card exactly as it was and
//! writes only the additions.

use super::model::{Address, Contact, ContactFields, DateEntry, Entry, Related};
use crate::duplicates::{email_key, phone_key};
use crate::search::normalize;

fn plain(s: &str) -> String {
    normalize(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `kept` with the entries of `others` whose key it does not have yet appended, never preferred.
/// None when nothing is added, so the list is left alone.
fn add_missing<T: Clone>(
    kept: &[T],
    others: &[&[T]],
    key: impl Fn(&T) -> String,
    unprefer: impl Fn(&mut T),
) -> Option<Vec<T>> {
    let mut out = kept.to_vec();
    let mut seen: Vec<String> = kept.iter().map(&key).collect();
    for list in others {
        for item in *list {
            let k = key(item);
            if seen.contains(&k) {
                continue;
            }
            seen.push(k);
            let mut item = item.clone();
            unprefer(&mut item);
            out.push(item);
        }
    }
    (out.len() > kept.len()).then_some(out)
}

fn entry_key(e: &Entry) -> String {
    e.value.trim().to_lowercase()
}

fn phone_entry_key(e: &Entry) -> String {
    phone_key(&e.value).unwrap_or_else(|| entry_key(e))
}

fn email_entry_key(e: &Entry) -> String {
    email_key(&e.value).unwrap_or_default()
}

fn address_key(a: &Address) -> String {
    [&a.street, &a.city, &a.region, &a.postcode, &a.country]
        .iter()
        .map(|p| plain(p.as_deref().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn unprefer_entry(e: &mut Entry) {
    e.preferred = false;
}

fn unprefer_address(a: &mut Address) {
    a.preferred = false;
}

/// The first value among `others` for a field the kept contact does not have.
fn fill(
    kept: &Option<String>,
    others: &[&Contact],
    get: impl Fn(&Contact) -> &Option<String>,
) -> Option<String> {
    if kept.is_some() {
        return None;
    }
    others.iter().find_map(|c| get(c).clone())
}

/// The fields that merge `others` into `kept`:
/// - emails, phones, URLs, addresses, dates and related names `kept` lacks are appended with
///   their labels (emails by address, phones by number in any format, dates by date, addresses
///   by all their parts, related names and URLs by text);
/// - organization, department, job title, nickname and birthday fill empty fields;
/// - name parts are taken from the first contact that has any, only when `kept` has none, and
///   the display name then stays what it was;
/// - each note `kept`'s note does not already contain is appended after a blank line.
pub fn merge_fields(kept: &Contact, others: &[&Contact]) -> ContactFields {
    let lists = |get: fn(&Contact) -> &Vec<Entry>| -> Vec<&[Entry]> {
        others.iter().map(|c| get(c).as_slice()).collect()
    };
    let mut f = ContactFields {
        emails: add_missing(
            &kept.emails,
            &lists(|c| &c.emails),
            email_entry_key,
            unprefer_entry,
        ),
        phones: add_missing(
            &kept.phones,
            &lists(|c| &c.phones),
            phone_entry_key,
            unprefer_entry,
        ),
        urls: add_missing(&kept.urls, &lists(|c| &c.urls), entry_key, unprefer_entry),
        addresses: add_missing(
            &kept.addresses,
            &others
                .iter()
                .map(|c| c.addresses.as_slice())
                .collect::<Vec<_>>(),
            address_key,
            unprefer_address,
        ),
        dates: add_missing(
            &kept.dates,
            &others
                .iter()
                .map(|c| c.dates.as_slice())
                .collect::<Vec<_>>(),
            |d: &DateEntry| d.date.clone(),
            |_| {},
        ),
        related: add_missing(
            &kept.related,
            &others
                .iter()
                .map(|c| c.related.as_slice())
                .collect::<Vec<_>>(),
            |r: &Related| plain(&r.name),
            |_| {},
        ),
        organization: fill(&kept.organization, others, |c| &c.organization),
        department: fill(&kept.department, others, |c| &c.department),
        job_title: fill(&kept.job_title, others, |c| &c.job_title),
        nickname: fill(&kept.nickname, others, |c| &c.nickname),
        birthday: fill(&kept.birthday, others, |c| &c.birthday),
        ..ContactFields::default()
    };
    let kept_name = [
        &kept.given_name,
        &kept.family_name,
        &kept.middle_name,
        &kept.prefix,
        &kept.suffix,
    ];
    if kept_name.iter().all(|p| p.is_none()) {
        let named = others.iter().find(|c| {
            [
                &c.given_name,
                &c.family_name,
                &c.middle_name,
                &c.prefix,
                &c.suffix,
            ]
            .iter()
            .any(|p| p.is_some())
        });
        if let Some(c) = named {
            f.given_name = c.given_name.clone();
            f.family_name = c.family_name.clone();
            f.middle_name = c.middle_name.clone();
            f.prefix = c.prefix.clone();
            f.suffix = c.suffix.clone();
            f.display_name = Some(kept.display_name.clone());
        }
    }
    let mut note = kept.note.clone().unwrap_or_default();
    for c in others {
        let Some(n) = c.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
            continue;
        };
        if note.contains(n) {
            continue;
        }
        if !note.is_empty() {
            note.push_str("\n\n");
        }
        note.push_str(n);
    }
    if note != kept.note.clone().unwrap_or_default() {
        f.note = Some(note);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(v: &str, label: Option<&str>, preferred: bool) -> Entry {
        Entry {
            value: v.into(),
            label: label.map(str::to_string),
            preferred,
        }
    }

    fn person(id: &str) -> Contact {
        Contact {
            id: id.into(),
            display_name: "Alex Example".into(),
            given_name: Some("Alex".into()),
            family_name: Some("Example".into()),
            ..Contact::default()
        }
    }

    #[test]
    fn missing_entries_are_appended_and_never_preferred() {
        let mut kept = person("k");
        kept.emails = vec![entry("alex@example.com", Some("home"), true)];
        kept.phones = vec![entry("+44 7700 900123", Some("mobile"), true)];
        let mut other = person("o");
        other.emails = vec![
            entry("ALEX@example.com", Some("work"), false),
            entry("alex.work@example.org", Some("work"), true),
        ];
        other.phones = vec![
            entry("07700 900123", None, false),
            entry("+1 202-555-0123", Some("boat"), true),
        ];
        other.urls = vec![entry("https://example.com/alex", Some("home page"), false)];
        let f = merge_fields(&kept, &[&other]);
        assert_eq!(
            f.emails.unwrap(),
            [
                entry("alex@example.com", Some("home"), true),
                entry("alex.work@example.org", Some("work"), false),
            ]
        );
        assert_eq!(
            f.phones.unwrap(),
            [
                entry("+44 7700 900123", Some("mobile"), true),
                entry("+1 202-555-0123", Some("boat"), false),
            ]
        );
        assert_eq!(
            f.urls.unwrap(),
            [entry("https://example.com/alex", Some("home page"), false)]
        );
        assert!(f.addresses.is_none() && f.dates.is_none() && f.related.is_none());
    }

    #[test]
    fn addresses_dates_and_related_names_are_merged_by_their_content() {
        let mut kept = person("k");
        kept.addresses = vec![Address {
            street: Some("1 Example Street".into()),
            city: Some("Exampleton".into()),
            preferred: true,
            ..Address::default()
        }];
        kept.dates = vec![DateEntry {
            date: "2010-06-01".into(),
            label: Some("anniversary".into()),
        }];
        let mut other = person("o");
        other.addresses = vec![
            Address {
                street: Some("1 example  street".into()),
                city: Some("EXAMPLETON".into()),
                label: Some("home".into()),
                ..Address::default()
            },
            Address {
                city: Some("Otherville".into()),
                preferred: true,
                ..Address::default()
            },
        ];
        other.dates = vec![
            DateEntry {
                date: "2010-06-01".into(),
                label: None,
            },
            DateEntry {
                date: "--03-04".into(),
                label: Some("other".into()),
            },
        ];
        other.related = vec![Related {
            name: "Sam Example".into(),
            label: Some("mother".into()),
        }];
        let f = merge_fields(&kept, &[&other]);
        let addresses = f.addresses.unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(addresses[1].city.as_deref(), Some("Otherville"));
        assert!(!addresses[1].preferred);
        let dates = f.dates.unwrap();
        assert_eq!(dates.len(), 2);
        assert_eq!(dates[1].date, "--03-04");
        assert_eq!(f.related.unwrap()[0].name, "Sam Example");
    }

    #[test]
    fn empty_single_fields_are_filled_and_set_ones_kept() {
        let mut kept = person("k");
        kept.organization = Some("Kept Ltd".into());
        let mut a = person("a");
        a.organization = Some("Other Ltd".into());
        a.job_title = Some("Tinkerer".into());
        let mut b = person("b");
        b.job_title = Some("Later".into());
        b.nickname = Some("Lex".into());
        b.birthday = Some("--05-12".into());
        b.department = Some("Research".into());
        let f = merge_fields(&kept, &[&a, &b]);
        assert_eq!(f.organization, None);
        assert_eq!(f.job_title.as_deref(), Some("Tinkerer"));
        assert_eq!(f.nickname.as_deref(), Some("Lex"));
        assert_eq!(f.birthday.as_deref(), Some("--05-12"));
        assert_eq!(f.department.as_deref(), Some("Research"));
        assert!(!f.changes_name() && f.display_name.is_none());
    }

    #[test]
    fn name_parts_come_over_only_when_the_kept_card_has_none() {
        let company = Contact {
            id: "c".into(),
            display_name: "Example Plumbing Co".into(),
            organization: Some("Example Plumbing Co".into()),
            ..Contact::default()
        };
        let mut other = person("o");
        other.prefix = Some("Dr.".into());
        let f = merge_fields(&company, &[&other]);
        assert_eq!(f.given_name.as_deref(), Some("Alex"));
        assert_eq!(f.family_name.as_deref(), Some("Example"));
        assert_eq!(f.prefix.as_deref(), Some("Dr."));
        assert_eq!(f.display_name.as_deref(), Some("Example Plumbing Co"));
        let f = merge_fields(&person("k"), &[&other]);
        assert!(!f.changes_name());
    }

    #[test]
    fn notes_are_appended_once() {
        let mut kept = person("k");
        kept.note = Some("Likes tea.".into());
        let mut a = person("a");
        a.note = Some("Likes tea.".into());
        let mut b = person("b");
        b.note = Some(" Met in 2019. ".into());
        let f = merge_fields(&kept, &[&a, &b]);
        assert_eq!(f.note.as_deref(), Some("Likes tea.\n\nMet in 2019."));
        let f = merge_fields(&person("k"), &[&b]);
        assert_eq!(f.note.as_deref(), Some("Met in 2019."));
        assert!(merge_fields(&kept, &[&a]).note.is_none());
    }

    #[test]
    fn nothing_new_gives_no_fields() {
        let mut kept = person("k");
        kept.emails = vec![entry("alex@example.com", None, false)];
        let mut other = person("o");
        other.emails = vec![entry("Alex@Example.com", None, true)];
        assert!(merge_fields(&kept, &[&other]).is_empty());
    }
}
