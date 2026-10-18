//! Local search over the indexed contacts: words matched without regard to case or accents, or a
//! phone number in any common format.

use crate::vcard::model::Contact;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lower case with accents removed: `Zoë Ångström` -> `zoe angstrom`.
pub fn normalize(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// Every word must appear in one of the contact's text fields.
    Words(Vec<String>),
    /// The digits of a phone number.
    Phone(String),
}

impl Query {
    /// A query of at least 6 digits (spaces and `+()-./` ignored) is a phone number; anything else
    /// is words. None for an empty query. A query without a letter or digit has no words and
    /// matches nobody.
    pub fn parse(q: &str) -> Option<Query> {
        let q = q.trim();
        if q.is_empty() {
            return None;
        }
        let compact: String = q
            .chars()
            .filter(|c| !c.is_whitespace() && !"+()-./".contains(*c))
            .collect();
        if compact.len() >= 6 && compact.chars().all(|c| c.is_ascii_digit()) {
            return Some(Query::Phone(compact));
        }
        if !q.chars().any(char::is_alphanumeric) {
            return Some(Query::Words(Vec::new()));
        }
        Some(Query::Words(
            normalize(q)
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        ))
    }
}

/// The same last 9 digits (national and international forms of one number), or the query's
/// digits without leading zeros inside the number.
fn phone_matches(query: &str, number: &str) -> bool {
    let d = digits(number);
    let last9 = |s: &str| s[s.len().saturating_sub(9)..].to_string();
    if query.len() >= 9 && d.len() >= 9 && last9(query) == last9(&d) {
        return true;
    }
    let stripped = query.trim_start_matches('0');
    stripped.len() >= 4 && d.contains(stripped)
}

fn texts(c: &Contact) -> Vec<String> {
    let mut out: Vec<&str> = vec![&c.display_name];
    out.extend(
        [
            &c.given_name,
            &c.family_name,
            &c.middle_name,
            &c.prefix,
            &c.suffix,
            &c.nickname,
            &c.organization,
            &c.department,
            &c.job_title,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str),
    );
    out.extend(c.emails.iter().map(|e| e.value.as_str()));
    out.extend(c.related.iter().map(|r| r.name.as_str()));
    out.into_iter().map(normalize).collect()
}

pub fn matches(c: &Contact, q: &Query) -> bool {
    match q {
        Query::Phone(p) => c.phones.iter().any(|n| phone_matches(p, &n.value)),
        Query::Words(words) => {
            let texts = texts(c);
            !words.is_empty()
                && words
                    .iter()
                    .all(|w| texts.iter().any(|t| t.contains(w.as_str())))
        }
    }
}

/// 0: the display name is the query; 1: it starts with the query; 2: one of its words starts
/// with a query word; 3: any other match.
pub fn rank(c: &Contact, q: &Query) -> u8 {
    let Query::Words(words) = q else {
        return 3;
    };
    let name = normalize(&c.display_name);
    let query = words.join(" ");
    if name == query {
        return 0;
    }
    if name.starts_with(&query) {
        return 1;
    }
    let word_start = name
        .split_whitespace()
        .any(|n| words.iter().any(|w| n.starts_with(w.as_str())));
    if word_start {
        return 2;
    }
    3
}

/// The contacts matching the query (all of them without one), best match first, then by name.
pub fn search<'a>(contacts: &'a [Contact], query: Option<&str>) -> Vec<&'a Contact> {
    let q = query.and_then(Query::parse);
    let mut found: Vec<(u8, String, &Contact)> = contacts
        .iter()
        .filter(|c| q.as_ref().is_none_or(|q| matches(c, q)))
        .map(|c| {
            (
                q.as_ref().map_or(3, |q| rank(c, q)),
                normalize(&c.display_name),
                c,
            )
        })
        .collect();
    found.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.id.cmp(&b.2.id))
    });
    found.into_iter().map(|(_, _, c)| c).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::model::{Entry, Related};

    fn contact(id: &str, name: &str) -> Contact {
        Contact {
            id: id.into(),
            display_name: name.into(),
            ..Contact::default()
        }
    }

    fn phone(value: &str) -> Entry {
        Entry {
            value: value.into(),
            ..Entry::default()
        }
    }

    fn ids(found: &[&Contact]) -> Vec<String> {
        found.iter().map(|c| c.id.clone()).collect()
    }

    #[test]
    fn case_and_accents_do_not_matter() {
        let all = [contact("z", "Zoë Ångström"), contact("o", "Other Person")];
        assert_eq!(ids(&search(&all, Some("zoe ANGSTROM"))), ["z"]);
        assert_eq!(ids(&search(&all, Some("Zoë"))), ["z"]);
    }

    #[test]
    fn every_word_must_match_some_field() {
        let mut alex = contact("a", "Alex Example");
        alex.organization = Some("Example Widgets".into());
        alex.job_title = Some("Chief Tinkerer".into());
        alex.nickname = Some("Lex".into());
        alex.department = Some("Research".into());
        alex.emails = vec![Entry {
            value: "alex@example.com".into(),
            ..Entry::default()
        }];
        alex.related = vec![Related {
            name: "Morgan Example".into(),
            label: Some("mother".into()),
        }];
        let all = [alex, contact("b", "Bea Sample")];
        for q in [
            "alex widgets",
            "tinkerer",
            "lex",
            "research",
            "alex@example",
            "morgan",
        ] {
            assert_eq!(ids(&search(&all, Some(q))), ["a"], "{q}");
        }
        assert!(search(&all, Some("alex nobody")).is_empty());
    }

    #[test]
    fn phone_numbers_match_in_any_common_format() {
        let mut uk = contact("uk", "UK Person");
        uk.phones = vec![phone("+44 7700 900123")];
        let mut national = contact("nat", "National Person");
        national.phones = vec![phone("07700 900456")];
        let mut us = contact("us", "US Person");
        us.phones = vec![phone("+1 202-555-0123")];
        let all = [uk, national, us];
        assert_eq!(ids(&search(&all, Some("07700 900123"))), ["uk"]);
        assert_eq!(ids(&search(&all, Some("+44 7700 900123"))), ["uk"]);
        assert_eq!(ids(&search(&all, Some("(0)7700-900.123"))), ["uk"]);
        assert_eq!(ids(&search(&all, Some("900123"))), ["uk"]);
        assert_eq!(ids(&search(&all, Some("+44 7700 900456"))), ["nat"]);
        assert_eq!(ids(&search(&all, Some("202 555 0123"))), ["us"]);
        assert_eq!(
            Query::parse("12345"),
            Some(Query::Words(vec!["12345".into()]))
        );
    }

    #[test]
    fn exact_names_come_first_then_prefixes_then_word_starts() {
        let mut pat = contact("4", "Pat Example");
        pat.emails = vec![Entry {
            value: "sam@example.com".into(),
            ..Entry::default()
        }];
        let all = [
            pat,
            contact("3", "Alex Samson"),
            contact("2", "Samuel Example"),
            contact("1", "Sam"),
        ];
        assert_eq!(ids(&search(&all, Some("sam"))), ["1", "2", "3", "4"]);
    }

    #[test]
    fn without_a_query_everyone_is_listed_by_name() {
        let all = [
            contact("b", "bea"),
            contact("a", "Alex"),
            contact("c", "Ćiro"),
        ];
        assert_eq!(ids(&search(&all, None)), ["a", "b", "c"]);
        assert_eq!(ids(&search(&all, Some("   "))), ["a", "b", "c"]);
    }

    #[test]
    fn a_query_without_letters_or_digits_matches_nobody() {
        let all = [contact("a", "Ann-Marie"), contact("b", "Bo")];
        for q in ["-", "+", "()", "...", "\u{301}"] {
            assert!(search(&all, Some(q)).is_empty(), "{q:?}");
        }
        assert_eq!(ids(&search(&all, Some("ann-"))), ["a"]);
    }
}
