//! Likely duplicate contacts: contacts that share an email address, a phone number or a full
//! name. Linked contacts form one group, also through a chain (A shares an email with B, B a
//! phone with C).

use crate::search::{digits, normalize};
use crate::vcard::model::Contact;
use std::collections::{BTreeMap, BTreeSet};

/// Why contacts were linked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    Email,
    Phone,
    Name,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Email => "email",
            Reason::Phone => "phone",
            Reason::Name => "name",
        }
    }
}

/// One set of likely duplicates: indexes into the contacts searched, best to keep first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub reasons: Vec<Reason>,
    pub members: Vec<usize>,
}

/// An email address trimmed and lower-cased; None when empty.
pub fn email_key(email: &str) -> Option<String> {
    let e = email.trim().to_lowercase();
    (!e.is_empty()).then_some(e)
}

/// The last 9 digits of a number with at least 9, else all digits of a number with 6 to 8. So a
/// national and an international form of one number match, and short codes match nothing.
pub fn phone_key(number: &str) -> Option<String> {
    let d = digits(number);
    match d.len() {
        n if n >= 9 => Some(d[n - 9..].to_string()),
        6..=8 => Some(d),
        _ => None,
    }
}

/// A display name without case, accents or extra spaces, for comparing and sorting.
fn plain_name(display_name: &str) -> String {
    let name = normalize(display_name);
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The plain display name when it has at least 2 words.
pub fn name_key(display_name: &str) -> Option<String> {
    let name = plain_name(display_name);
    (name.split(' ').count() >= 2).then_some(name)
}

/// How much a contact holds: every filled field plus every list entry.
pub fn fields_filled(c: &Contact) -> usize {
    let scalars = [
        &c.given_name,
        &c.family_name,
        &c.middle_name,
        &c.prefix,
        &c.suffix,
        &c.nickname,
        &c.organization,
        &c.department,
        &c.job_title,
        &c.birthday,
        &c.note,
    ];
    scalars.iter().filter(|s| s.is_some()).count()
        + c.emails.len()
        + c.phones.len()
        + c.addresses.len()
        + c.urls.len()
        + c.dates.len()
        + c.related.len()
        + c.social.len()
        + c.im.len()
}

fn keys(c: &Contact) -> Vec<(Reason, String)> {
    let mut out: Vec<(Reason, String)> = Vec::new();
    out.extend(
        c.emails
            .iter()
            .filter_map(|e| email_key(&e.value))
            .map(|k| (Reason::Email, k)),
    );
    out.extend(
        c.phones
            .iter()
            .filter_map(|p| phone_key(&p.value))
            .map(|k| (Reason::Phone, k)),
    );
    out.extend(name_key(&c.display_name).map(|k| (Reason::Name, k)));
    out
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The groups of likely duplicates among these contacts (groups and unparseable cards must not be
/// passed). Members are ordered with a photo first, then most fields filled, then by name and id;
/// groups by their first member's name and id.
pub fn find(contacts: &[Contact]) -> Vec<DuplicateGroup> {
    let mut by_key: BTreeMap<(Reason, String), BTreeSet<usize>> = BTreeMap::new();
    for (i, c) in contacts.iter().enumerate() {
        for key in keys(c) {
            by_key.entry(key).or_default().insert(i);
        }
    }
    let mut parent: Vec<usize> = (0..contacts.len()).collect();
    let mut links: Vec<(Reason, usize)> = Vec::new();
    for ((reason, _), members) in &by_key {
        if members.len() < 2 {
            continue;
        }
        let mut it = members.iter();
        let first = *it.next().unwrap_or(&0);
        for &m in it {
            let (a, b) = (root(&mut parent, first), root(&mut parent, m));
            if a != b {
                parent[b] = a;
            }
        }
        links.push((*reason, first));
    }
    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..contacts.len() {
        let r = root(&mut parent, i);
        components.entry(r).or_default().push(i);
    }
    let mut reasons: BTreeMap<usize, BTreeSet<Reason>> = BTreeMap::new();
    for (reason, member) in links {
        let r = root(&mut parent, member);
        reasons.entry(r).or_default().insert(reason);
    }
    let order = |i: &usize| {
        let c = &contacts[*i];
        (
            !c.has_photo,
            usize::MAX - fields_filled(c),
            plain_name(&c.display_name),
            c.id.clone(),
        )
    };
    let mut groups: Vec<DuplicateGroup> = components
        .into_iter()
        .filter(|(_, m)| m.len() >= 2)
        .map(|(r, mut members)| {
            members.sort_by_key(order);
            DuplicateGroup {
                reasons: reasons.remove(&r).unwrap_or_default().into_iter().collect(),
                members,
            }
        })
        .collect();
    groups.sort_by_key(|g| {
        let c = &contacts[g.members[0]];
        (plain_name(&c.display_name), c.id.clone())
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::model::Entry;

    fn contact(id: &str, name: &str, emails: &[&str], phones: &[&str]) -> Contact {
        let entry = |v: &&str| Entry {
            value: v.to_string(),
            ..Entry::default()
        };
        Contact {
            id: id.into(),
            display_name: name.into(),
            emails: emails.iter().map(entry).collect(),
            phones: phones.iter().map(entry).collect(),
            ..Contact::default()
        }
    }

    fn ids(contacts: &[Contact], groups: &[DuplicateGroup]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|g| g.members.iter().map(|&i| contacts[i].id.clone()).collect())
            .collect()
    }

    #[test]
    fn keys_normalise_emails_phones_and_names() {
        assert_eq!(
            email_key(" Alex@Example.COM ").as_deref(),
            Some("alex@example.com")
        );
        assert_eq!(email_key("  "), None);
        assert_eq!(phone_key("+44 7700 900123").as_deref(), Some("700900123"));
        assert_eq!(phone_key("07700 900123").as_deref(), Some("700900123"));
        assert_eq!(phone_key("555-0123"), Some("5550123".into()));
        assert_eq!(phone_key("112"), None);
        assert_eq!(name_key("Zoë  Example").as_deref(), Some("zoe example"));
        assert_eq!(name_key("Cher"), None);
    }

    #[test]
    fn shared_emails_phones_and_names_link_contacts() {
        let all = [
            contact("a", "Alex Example", &["alex@example.com"], &[]),
            contact("b", "A. Example", &["ALEX@example.com"], &[]),
            contact("c", "Sam Example", &[], &["+44 7700 900123"]),
            contact("d", "Samuel Example", &[], &["07700 900123"]),
            contact("e", "Robin Example", &[], &[]),
            contact("f", "robin  EXAMPLE", &[], &[]),
            contact(
                "g",
                "Unique Person",
                &["unique@example.com"],
                &["+1 202-555-0199"],
            ),
        ];
        let groups = find(&all);
        assert_eq!(ids(&all, &groups), [["b", "a"], ["e", "f"], ["c", "d"]]);
        assert_eq!(groups[0].reasons, [Reason::Email]);
        assert_eq!(groups[1].reasons, [Reason::Name]);
        assert_eq!(groups[2].reasons, [Reason::Phone]);
    }

    #[test]
    fn links_are_transitive_and_reasons_are_collected() {
        let all = [
            contact("a", "Alex Example", &["alex@example.com"], &[]),
            contact("b", "Lex", &["alex@example.com"], &["+44 7700 900456"]),
            contact("c", "Al", &[], &["07700 900456"]),
        ];
        let groups = find(&all);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].reasons, [Reason::Email, Reason::Phone]);
        assert_eq!(groups[0].members.len(), 3);
    }

    #[test]
    fn one_word_names_short_numbers_and_one_contact_twice_link_nothing() {
        let all = [
            contact("a", "Cher", &[], &["112"]),
            contact("b", "Cher", &[], &["112"]),
            contact(
                "c",
                "Pat Example",
                &["pat@example.com", "PAT@example.com"],
                &[],
            ),
        ];
        assert!(find(&all).is_empty());
    }

    #[test]
    fn the_contact_to_keep_comes_first() {
        let mut photo = contact("z", "Alex Example", &["alex@example.com"], &[]);
        photo.has_photo = true;
        let mut rich = contact(
            "y",
            "Alex Example",
            &["alex@example.com"],
            &["+44 7700 900123"],
        );
        rich.note = Some("x".into());
        let plain = contact("x", "Alex Example", &["alex@example.com"], &[]);
        let all = [plain, rich, photo];
        assert_eq!(ids(&all, &find(&all)), [["z", "y", "x"]]);
    }
}
