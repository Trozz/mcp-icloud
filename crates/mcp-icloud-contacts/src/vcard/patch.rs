//! Changing cards. Only the lines a change touches are rewritten; every other line keeps its
//! exact original text, so photos, Apple blobs and unknown properties survive edits.

use super::build::property;
use super::labels::{Field, Written, write_label};
use super::model::{Address, ContactFields, DateEntry, Entry, Related};
use super::parse::{self, GROUP_KIND, GROUP_MEMBER, joined_name, member_uid, named};
use crate::error::{ContactsError, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use mcp_icloud_dav::line::{Component, Property, escape_text, unescape_text};
use serde::Serialize;
use std::collections::BTreeSet;

/// Properties that hold one value of a list. Other lines in the same `itemN` group (its
/// `X-ABLabel`, `X-ABADR`, ...) belong to that value.
const LIST_PROPERTIES: [&str; 8] = [
    "EMAIL",
    "TEL",
    "URL",
    "ADR",
    "X-ABDATE",
    "X-ABRELATEDNAMES",
    "IMPP",
    "X-SOCIALPROFILE",
];

fn invalid(msg: impl Into<String>) -> ContactsError {
    ContactsError::invalid(msg)
}

/// Applies the given fields to a contact card. Validation errors name the field, never its value.
pub fn apply(card: &mut Component, f: &ContactFields) -> Result<()> {
    validate(f)?;
    let mut numbers = ItemNumbers::new(card);
    let before = (n_value(card), org_name(card));
    if f.changes_name() {
        set_name(card, f);
    }
    if f.organization.is_some() || f.department.is_some() {
        set_org(card, f);
    }
    if let Some(v) = &f.nickname {
        set_text(card, "NICKNAME", v);
    }
    if let Some(v) = &f.job_title {
        set_text(card, "TITLE", v);
    }
    if let Some(v) = &f.note {
        set_text(card, "NOTE", v);
    }
    if let Some(v) = &f.birthday {
        set_birthday(card, v)?;
    }
    let rebuild = match f.display_name.as_deref().map(str::trim) {
        Some("") => true,
        Some(v) => {
            set_text(card, "FN", v);
            false
        }
        None => {
            before.0 != n_value(card)
                || (person_name(card).is_empty() && before.1 != org_name(card))
        }
    };
    if rebuild {
        rebuild_display_name(card)?;
    }
    if let Some(list) = &f.emails {
        let old = parse::entries(card, Field::Email);
        replace(card, list, old, &mut numbers, |e, n| {
            render_entry(Field::Email, e, n)
        })?;
    }
    if let Some(list) = &f.phones {
        let old = parse::entries(card, Field::Phone);
        replace(card, list, old, &mut numbers, |e, n| {
            render_entry(Field::Phone, e, n)
        })?;
    }
    if let Some(list) = &f.urls {
        let old = parse::entries(card, Field::Url);
        replace(card, list, old, &mut numbers, |e, n| {
            render_entry(Field::Url, e, n)
        })?;
    }
    if let Some(list) = &f.addresses {
        let old = parse::addresses(card);
        replace(card, list, old, &mut numbers, render_address)?;
    }
    if let Some(list) = &f.dates {
        let old = parse::dates(card);
        replace(card, list, old, &mut numbers, render_date)?;
    }
    if let Some(list) = &f.related {
        let old = parse::related(card);
        replace(card, list, old, &mut numbers, render_related)?;
    }
    Ok(())
}

/// Sets `REV` to now, in place when the card has one.
pub fn touch(card: &mut Component, now: DateTime<Utc>) {
    let rev = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let at = named(card, "REV").map(|(i, _)| i).next();
    match at {
        Some(i) => card.properties[i].set_value(rev),
        None => card.properties.push(Property::new("REV", rev)),
    }
}

/// Fails when a serialised card is larger than iCloud accepts.
pub fn check_size(text: &str, max: u64) -> Result<()> {
    let len = text.len() as u64;
    if len > max {
        return Err(invalid(format!(
            "the card would be {len} bytes; iCloud accepts at most {max}"
        )));
    }
    Ok(())
}

/// Renames a group: `N` and `FN`.
pub fn rename_group(card: &mut Component, name: &str) {
    let value = escape_text(name.trim());
    let at = named(card, "N").map(|(i, _)| i).next();
    match at {
        Some(i) if card.properties[i].value() != value => card.properties[i].set_value(value),
        Some(_) => {}
        None => card.properties.push(Property::new("N", value)),
    }
    set_text(card, "FN", name);
}

/// Adds member lines for UIDs the group does not list yet, after its last member line.
pub fn add_members(card: &mut Component, uids: &[String]) {
    let mut present: Vec<String> = named(card, GROUP_MEMBER)
        .filter_map(|(_, p)| member_uid(p.value()))
        .collect();
    let mut at = named(card, GROUP_MEMBER)
        .map(|(i, _)| i + 1)
        .last()
        .or_else(|| named(card, GROUP_KIND).map(|(i, _)| i + 1).next())
        .unwrap_or(card.properties.len());
    for uid in uids {
        if present.contains(uid) {
            continue;
        }
        card.properties
            .insert(at, Property::new(GROUP_MEMBER, format!("urn:uuid:{uid}")));
        present.push(uid.clone());
        at += 1;
    }
}

/// Removes the member lines of these UIDs, in either the `urn:uuid:` or the bare form.
pub fn remove_members(card: &mut Component, uids: &[String]) {
    card.properties.retain(|p| {
        !(p.base_name().eq_ignore_ascii_case(GROUP_MEMBER)
            && member_uid(p.value()).is_some_and(|u| uids.contains(&u)))
    });
}

/// Sets, changes or (with "") removes a single text property. An unchanged value keeps its line.
pub fn set_text(card: &mut Component, name: &str, value: &str) {
    let value = value.trim();
    let idx: Vec<usize> = named(card, name).map(|(i, _)| i).collect();
    if value.is_empty() {
        remove_indexes(card, &idx);
        return;
    }
    match idx.first() {
        Some(&i) if unescape_text(card.properties[i].value()).trim() == value => {}
        Some(&i) => {
            card.properties[i].set_value(escape_text(value));
            remove_indexes(card, &idx[1..]);
        }
        None => card
            .properties
            .push(Property::new(name, escape_text(value))),
    }
}

fn remove_indexes(card: &mut Component, idx: &[usize]) {
    let remove: BTreeSet<usize> = idx.iter().copied().collect();
    let mut i = 0;
    card.properties.retain(|_| {
        let keep = !remove.contains(&i);
        i += 1;
        keep
    });
}

/// Splits a structured value at unescaped `;`, keeping each component's escapes.
fn raw_components(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in value.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                cur.push(c);
                escaped = true;
            }
            ';' => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn set_name(card: &mut Component, f: &ContactFields) {
    let at = named(card, "N").map(|(i, _)| i).next();
    let mut raw = at
        .map(|i| raw_components(card.properties[i].value()))
        .unwrap_or_default();
    raw.resize(5.max(raw.len()), String::new());
    let parts = [
        &f.family_name,
        &f.given_name,
        &f.middle_name,
        &f.prefix,
        &f.suffix,
    ];
    for (k, v) in parts.iter().enumerate() {
        if let Some(v) = v {
            raw[k] = escape_text(v.trim());
        }
    }
    let value = raw.join(";");
    match at {
        Some(i) if card.properties[i].value() != value => card.properties[i].set_value(value),
        Some(_) => {}
        None => card.properties.push(Property::new("N", value)),
    }
}

fn set_org(card: &mut Component, f: &ContactFields) {
    let at = named(card, "ORG").map(|(i, _)| i).next();
    let mut raw = at
        .map(|i| raw_components(card.properties[i].value()))
        .unwrap_or_default();
    if raw.is_empty() {
        raw.push(String::new());
    }
    if let Some(o) = &f.organization {
        raw[0] = escape_text(o.trim());
    }
    if let Some(d) = &f.department {
        raw.truncate(1);
        if !d.trim().is_empty() {
            raw.push(escape_text(d.trim()));
        }
    }
    let empty = raw.iter().all(|r| r.is_empty());
    match at {
        Some(i) if empty => {
            card.properties.remove(i);
        }
        Some(i) => {
            let value = raw.join(";");
            if card.properties[i].value() != value {
                card.properties[i].set_value(value);
            }
        }
        None if empty => {}
        None => card.properties.push(Property::new("ORG", raw.join(";"))),
    }
}

/// The structured name joined like iOS shows it ("Dr. Alex J. Example Jr.").
fn person_name(card: &Component) -> String {
    let Some((_, n)) = named(card, "N").next() else {
        return String::new();
    };
    let mut parts = parse::components(n.value());
    parts.resize(5, String::new());
    let opt = |s: &String| Some(s.clone());
    joined_name([
        &opt(&parts[3]),
        &opt(&parts[1]),
        &opt(&parts[2]),
        &opt(&parts[0]),
        &opt(&parts[4]),
    ])
}

fn n_value(card: &Component) -> Option<String> {
    named(card, "N").next().map(|(_, p)| p.value().to_string())
}

fn org_name(card: &Component) -> Option<String> {
    named(card, "ORG")
        .next()
        .and_then(|(_, p)| parse::components(p.value()).into_iter().next())
}

fn rebuild_display_name(card: &mut Component) -> Result<()> {
    let mut name = person_name(card);
    if name.is_empty() {
        name = named(card, "ORG")
            .find_map(|(_, p)| parse::components(p.value()).into_iter().next())
            .map(|o| o.trim().to_string())
            .unwrap_or_default();
    }
    if name.is_empty() {
        return Err(invalid("a contact needs a name or an organization"));
    }
    set_text(card, "FN", &name);
    Ok(())
}

/// `YYYY-MM-DD` gives the year; `--MM-DD` gives none.
fn input_date(s: &str) -> Option<(Option<i32>, u32, u32)> {
    let s = s.trim();
    if let Some(md) = s.strip_prefix("--") {
        let (m, d) = md.split_once('-')?;
        if m.len() != 2 || d.len() != 2 {
            return None;
        }
        let (m, d) = (m.parse().ok()?, d.parse().ok()?);
        NaiveDate::from_ymd_opt(2000, m, d)?;
        return Some((None, m, d));
    }
    if s.len() != 10 {
        return None;
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some((Some(date.year()), date.month(), date.day()))
}

/// A date as shown (`2010-06-01`, `--06-01`) and the parameter and value it is written with.
fn date_line(date: &str) -> Option<(String, &'static str, String)> {
    let (year, m, d) = input_date(date)?;
    Some(match year {
        Some(y) => (
            format!("{y:04}-{m:02}-{d:02}"),
            "",
            format!("{y:04}-{m:02}-{d:02}"),
        ),
        None => (
            format!("--{m:02}-{d:02}"),
            ";X-APPLE-OMIT-YEAR=1604",
            format!("1604-{m:02}-{d:02}"),
        ),
    })
}

fn set_birthday(card: &mut Component, value: &str) -> Result<()> {
    let idx: Vec<usize> = named(card, "BDAY").map(|(i, _)| i).collect();
    if value.trim().is_empty() {
        remove_indexes(card, &idx);
        return Ok(());
    }
    let (shown, omit, written) = date_line(value)
        .ok_or_else(|| invalid("birthday must be YYYY-MM-DD, or --MM-DD without a year"))?;
    let current = parse::parse_contact(card, "").birthday;
    if current.as_deref() == Some(shown.as_str()) {
        return Ok(());
    }
    let line = if omit.is_empty() {
        format!("BDAY;VALUE=date:{written}")
    } else {
        format!("BDAY{omit}:{written}")
    };
    let new = property(&line)?;
    let at = idx.first().copied();
    remove_indexes(card, &idx);
    let at = at
        .unwrap_or(card.properties.len())
        .min(card.properties.len());
    card.properties.insert(at, new);
    Ok(())
}

fn is_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(char::is_whitespace)
}

fn one_preferred(name: &str, preferred: impl Iterator<Item = bool>) -> Result<()> {
    if preferred.filter(|p| *p).count() > 1 {
        return Err(invalid(format!("at most one of {name} may be preferred")));
    }
    Ok(())
}

fn validate(f: &ContactFields) -> Result<()> {
    for (name, list) in [
        ("emails", &f.emails),
        ("phones", &f.phones),
        ("urls", &f.urls),
    ] {
        let Some(list) = list else { continue };
        for (i, e) in list.iter().enumerate() {
            if e.value.trim().is_empty() {
                return Err(invalid(format!("{name}[{i}].value is empty")));
            }
            if name == "emails" && !is_email(&e.value) {
                return Err(invalid(format!(
                    "emails[{i}].value is not an email address"
                )));
            }
        }
        one_preferred(name, list.iter().map(|e| e.preferred))?;
    }
    if let Some(list) = &f.addresses {
        for (i, a) in list.iter().enumerate() {
            let parts = [&a.street, &a.city, &a.region, &a.postcode, &a.country];
            if parts
                .iter()
                .all(|p| p.as_deref().is_none_or(|s| s.trim().is_empty()))
            {
                return Err(invalid(format!("addresses[{i}] is empty")));
            }
        }
        one_preferred("addresses", list.iter().map(|a| a.preferred))?;
    }
    if let Some(list) = &f.dates {
        for (i, d) in list.iter().enumerate() {
            if input_date(&d.date).is_none() {
                return Err(invalid(format!(
                    "dates[{i}].date must be YYYY-MM-DD, or --MM-DD without a year"
                )));
            }
        }
    }
    if let Some(list) = &f.related {
        for (i, r) in list.iter().enumerate() {
            if r.name.trim().is_empty() {
                return Err(invalid(format!("related[{i}].name is empty")));
            }
        }
    }
    if let Some(b) = &f.birthday
        && !b.trim().is_empty()
        && input_date(b).is_none()
    {
        return Err(invalid(
            "birthday must be YYYY-MM-DD, or --MM-DD without a year",
        ));
    }
    Ok(())
}

/// The `itemN` group numbers in use, and the next free one.
struct ItemNumbers {
    used: BTreeSet<u32>,
}

impl ItemNumbers {
    fn new(card: &Component) -> Self {
        let used = card
            .properties
            .iter()
            .filter_map(|p| p.group())
            .filter_map(|g| {
                let (prefix, n) = g.split_at_checked(4)?;
                prefix
                    .eq_ignore_ascii_case("item")
                    .then(|| n.parse::<u32>().ok())?
            })
            .collect();
        ItemNumbers { used }
    }

    /// The smallest `itemN` not used anywhere in the card before this change.
    fn next(&mut self) -> String {
        let n = (1..).find(|n| !self.used.contains(n)).unwrap_or(u32::MAX);
        self.used.insert(n);
        format!("item{n}")
    }
}

/// The line at `i` plus the lines of its `itemN` group that are not list values themselves.
fn with_companions(card: &Component, i: usize) -> Vec<usize> {
    let mut out = vec![i];
    if let Some(g) = card.properties[i].group() {
        for (k, p) in card.properties.iter().enumerate() {
            let same = p.group().is_some_and(|x| x.eq_ignore_ascii_case(g));
            let value = LIST_PROPERTIES
                .iter()
                .any(|l| p.base_name().eq_ignore_ascii_case(l));
            if k != i && same && !value {
                out.push(k);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Replaces one list. Entries equal to an existing entry (as shown) keep that entry's lines;
/// the others are written fresh. The list goes where its first line was, else at the end.
fn replace<T: Serialize>(
    card: &mut Component,
    wanted: &[T],
    old: Vec<(usize, T)>,
    numbers: &mut ItemNumbers,
    render: impl Fn(&T, &mut ItemNumbers) -> Result<Vec<Property>>,
) -> Result<()> {
    let shown = |v: &T| serde_json::to_value(v).unwrap_or_default();
    let old: Vec<(usize, serde_json::Value)> = old.iter().map(|(i, v)| (*i, shown(v))).collect();
    let mut used = vec![false; old.len()];
    let mut lines = Vec::new();
    for w in wanted {
        let want = shown(w);
        let found = (0..old.len()).find(|&k| !used[k] && old[k].1 == want);
        match found {
            Some(k) => {
                used[k] = true;
                for j in with_companions(card, old[k].0) {
                    lines.push(card.properties[j].clone());
                }
            }
            None => lines.extend(render(w, numbers)?),
        }
    }
    let remove: BTreeSet<usize> = old
        .iter()
        .flat_map(|(i, _)| with_companions(card, *i))
        .collect();
    let first = remove.iter().next().copied();
    let mut props: Vec<Property> = std::mem::take(&mut card.properties)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !remove.contains(i))
        .map(|(_, p)| p)
        .collect();
    let at = first.unwrap_or(props.len()).min(props.len());
    props.splice(at..at, lines);
    card.properties = props;
    Ok(())
}

/// The lines of one new list value: `[itemN.]PROP;type=...[;type=pref]<extra>:value`, plus an
/// `itemN.X-ABLabel` line when the label is an Apple code or custom text.
fn render_line(
    field: Field,
    extra: &str,
    value: &str,
    preferred: bool,
    label: Option<&str>,
    numbers: &mut ItemNumbers,
) -> Result<Vec<Property>> {
    let pref = if preferred { ";type=pref" } else { "" };
    let name = field.property();
    match write_label(field, label) {
        Written::Types(types) => {
            let t: String = types.iter().map(|t| format!(";type={t}")).collect();
            Ok(vec![property(&format!("{name}{t}{pref}{extra}:{value}"))?])
        }
        Written::Label(label) => {
            let g = numbers.next();
            let internet = if field == Field::Email {
                ";type=INTERNET"
            } else {
                ""
            };
            Ok(vec![
                property(&format!("{g}.{name}{internet}{pref}{extra}:{value}"))?,
                property(&format!("{g}.X-ABLabel:{}", escape_text(&label)))?,
            ])
        }
    }
}

fn render_entry(field: Field, e: &Entry, n: &mut ItemNumbers) -> Result<Vec<Property>> {
    render_line(
        field,
        "",
        &escape_text(e.value.trim()),
        e.preferred,
        e.label.as_deref(),
        n,
    )
}

fn render_address(a: &Address, n: &mut ItemNumbers) -> Result<Vec<Property>> {
    let part = |v: &Option<String>| escape_text(v.as_deref().unwrap_or_default().trim());
    let value = [
        String::new(),
        String::new(),
        part(&a.street),
        part(&a.city),
        part(&a.region),
        part(&a.postcode),
        part(&a.country),
    ]
    .join(";");
    render_line(
        Field::Address,
        "",
        &value,
        a.preferred,
        a.label.as_deref(),
        n,
    )
}

fn render_date(d: &DateEntry, n: &mut ItemNumbers) -> Result<Vec<Property>> {
    let (_, extra, written) = date_line(&d.date)
        .ok_or_else(|| invalid("dates must be YYYY-MM-DD, or --MM-DD without a year"))?;
    render_line(Field::Date, extra, &written, false, d.label.as_deref(), n)
}

fn render_related(r: &Related, n: &mut ItemNumbers) -> Result<Vec<Property>> {
    render_line(
        Field::Related,
        "",
        &escape_text(r.name.trim()),
        false,
        r.label.as_deref(),
        n,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::parse::{parse_card, parse_contact, parse_group};
    use chrono::TimeZone;

    const PERSON: &str = include_str!("../../tests/fixtures/apple-person.vcf");
    const COMPANY: &str = include_str!("../../tests/fixtures/apple-company.vcf");
    const GROUP: &str = include_str!("../../tests/fixtures/apple-group.vcf");

    fn card(text: &str) -> Component {
        parse_card(&text.replace('\n', "\r\n")).unwrap()
    }

    /// Physical lines removed from and added to a card's text by a change.
    fn diff(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
        let a: Vec<&str> = before.split("\r\n").collect();
        let b: Vec<&str> = after.split("\r\n").collect();
        let removed = a
            .iter()
            .filter(|l| !b.contains(l))
            .map(|l| l.to_string())
            .collect();
        let added = b
            .iter()
            .filter(|l| !a.contains(l))
            .map(|l| l.to_string())
            .collect();
        (removed, added)
    }

    fn change(text: &str, f: ContactFields) -> (Vec<String>, Vec<String>) {
        let mut c = card(text);
        let before = c.to_ics();
        apply(&mut c, &f).unwrap();
        diff(&before, &c.to_ics())
    }

    fn entry(value: &str, label: Option<&str>, preferred: bool) -> Entry {
        Entry {
            value: value.into(),
            label: label.map(str::to_string),
            preferred,
        }
    }

    #[test]
    fn changing_one_scalar_rewrites_only_its_line() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                nickname: Some("Al".into()),
                ..ContactFields::default()
            },
        );
        assert_eq!(removed, ["NICKNAME:Lex"]);
        assert_eq!(added, ["NICKNAME:Al"]);
    }

    #[test]
    fn clearing_a_scalar_removes_only_its_line() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                job_title: Some(String::new()),
                ..ContactFields::default()
            },
        );
        assert_eq!(removed, ["TITLE:Chief Tinkerer"]);
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn an_unchanged_value_keeps_its_line() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                nickname: Some("Lex".into()),
                birthday: Some("--05-12".into()),
                organization: Some("Example Widgets Ltd".into()),
                ..ContactFields::default()
            },
        );
        assert!(
            removed.is_empty() && added.is_empty(),
            "{removed:?} {added:?}"
        );
    }

    #[test]
    fn replacing_a_list_keeps_the_exact_lines_of_unchanged_entries() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                phones: Some(vec![
                    entry("+44 7700 900123", Some("mobile"), true),
                    entry("07700 900456", Some("home"), false),
                    entry("07700 900999", Some("summer house"), false),
                ]),
                ..ContactFields::default()
            },
        );
        assert_eq!(
            removed,
            ["item3.TEL:+1 202-555-0123", "item3.X-ABLabel:boat"]
        );
        assert_eq!(
            added,
            ["item9.TEL:07700 900999", "item9.X-ABLabel:summer house"]
        );
    }

    #[test]
    fn a_kept_entry_keeps_its_companion_lines() {
        let mut c = card(PERSON);
        let before = c.to_ics();
        let f = ContactFields {
            addresses: Some(vec![Address {
                street: Some("1 Example Street\nFlat 2".into()),
                city: Some("Exampleton".into()),
                region: Some("Example County".into()),
                postcode: Some("EX1 2AB".into()),
                country: Some("United Kingdom".into()),
                label: Some("home".into()),
                preferred: true,
            }]),
            ..ContactFields::default()
        };
        apply(&mut c, &f).unwrap();
        assert_eq!(c.to_ics(), before);
    }

    #[test]
    fn removing_an_entry_removes_its_group() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                addresses: Some(Vec::new()),
                ..ContactFields::default()
            },
        );
        assert_eq!(
            removed,
            [
                "item4.ADR;type=HOME;type=pref:;;1 Example Street\\nFlat 2;Exampleton;Example",
                "  County;EX1 2AB;United Kingdom",
                "item4.X-ABADR:gb",
                "item4.X-APPLE-SUBLOCALITY:Old Town",
            ]
        );
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn new_entries_use_the_label_table() {
        let mut c = card(COMPANY);
        apply(
            &mut c,
            &ContactFields {
                emails: Some(vec![
                    entry("hello@example.net", Some("work"), true),
                    entry("sales@example.net", Some("sales"), false),
                ]),
                ..ContactFields::default()
            },
        )
        .unwrap();
        let text = c.to_ics();
        assert!(
            text.contains("\r\nEMAIL;type=INTERNET;type=WORK;type=pref:hello@example.net\r\nitem2.EMAIL;type=INTERNET:sales@example.net\r\nitem2.X-ABLabel:sales\r\n"),
            "{text}"
        );
        let shown = parse_contact(&c, "x");
        assert_eq!(
            shown.emails,
            [
                entry("hello@example.net", Some("work"), true),
                entry("sales@example.net", Some("sales"), false),
            ]
        );
    }

    #[test]
    fn a_name_change_rebuilds_the_display_name() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                given_name: Some("Alexandra".into()),
                ..ContactFields::default()
            },
        );
        assert_eq!(
            removed,
            ["N:Example;Alex;J.;Dr.;Jr.", "FN:Dr. Alex J. Example Jr."]
        );
        assert_eq!(
            added,
            [
                "N:Example;Alexandra;J.;Dr.;Jr.",
                "FN:Dr. Alexandra J. Example Jr."
            ]
        );
    }

    #[test]
    fn resending_an_unchanged_name_keeps_a_custom_display_name() {
        let text = PERSON.replace("FN:Dr. Alex J. Example Jr.", "FN:Lexie");
        let (removed, added) = change(
            &text,
            ContactFields {
                given_name: Some("Alex".into()),
                family_name: Some("Example".into()),
                ..ContactFields::default()
            },
        );
        assert!(removed.is_empty(), "{removed:?}");
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn resending_an_unchanged_organization_keeps_a_custom_display_name() {
        let text = COMPANY.replace("FN:Example Plumbing Co", "FN:Example Pipes");
        let (removed, added) = change(
            &text,
            ContactFields {
                organization: Some("Example Plumbing Co".into()),
                ..ContactFields::default()
            },
        );
        assert!(removed.is_empty(), "{removed:?}");
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn a_given_display_name_wins() {
        let (_, added) = change(
            PERSON,
            ContactFields {
                given_name: Some("Alexandra".into()),
                display_name: Some("Lexie".into()),
                ..ContactFields::default()
            },
        );
        assert_eq!(added, ["N:Example;Alexandra;J.;Dr.;Jr.", "FN:Lexie"]);
    }

    #[test]
    fn renaming_a_company_renames_its_display_name() {
        let (removed, added) = change(
            COMPANY,
            ContactFields {
                organization: Some("Example Pipes Co".into()),
                ..ContactFields::default()
            },
        );
        assert_eq!(
            removed,
            ["FN:Example Plumbing Co", "ORG:Example Plumbing Co;"]
        );
        assert_eq!(added, ["FN:Example Pipes Co", "ORG:Example Pipes Co;"]);
    }

    #[test]
    fn clearing_every_name_is_refused() {
        let mut c = card(COMPANY);
        let e = apply(
            &mut c,
            &ContactFields {
                organization: Some(String::new()),
                ..ContactFields::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.kind(), "validation");
    }

    #[test]
    fn birthdays_are_written_with_or_without_a_year() {
        let (removed, added) = change(
            PERSON,
            ContactFields {
                birthday: Some("1990-05-12".into()),
                ..ContactFields::default()
            },
        );
        assert_eq!(removed, ["BDAY;X-APPLE-OMIT-YEAR=1604:1604-05-12"]);
        assert_eq!(added, ["BDAY;VALUE=date:1990-05-12"]);
        let (removed, _) = change(
            PERSON,
            ContactFields {
                birthday: Some(String::new()),
                ..ContactFields::default()
            },
        );
        assert_eq!(removed, ["BDAY;X-APPLE-OMIT-YEAR=1604:1604-05-12"]);
    }

    #[test]
    fn rev_is_set_and_prodid_and_show_as_are_kept() {
        let mut c = card(COMPANY);
        touch(&mut c, Utc.with_ymd_and_hms(2026, 10, 18, 9, 0, 0).unwrap());
        let text = c.to_ics();
        assert!(text.contains("\r\nREV:2026-10-18T09:00:00Z\r\n"), "{text}");
        assert!(text.contains("\r\nPRODID:-//Apple Inc.//macOS 26.1//EN\r\n"));
        assert!(text.contains("\r\nX-ABShowAs:COMPANY\r\n"));
    }

    #[test]
    fn invalid_input_is_refused_without_echoing_it() {
        let cases = [
            ContactFields {
                emails: Some(vec![entry("zz-invalid-value", None, false)]),
                ..ContactFields::default()
            },
            ContactFields {
                phones: Some(vec![
                    entry("07700 900111", None, true),
                    entry("07700 900222", None, true),
                ]),
                ..ContactFields::default()
            },
            ContactFields {
                birthday: Some("12/05/1990".into()),
                ..ContactFields::default()
            },
            ContactFields {
                dates: Some(vec![DateEntry {
                    date: "--13-01".into(),
                    label: None,
                }]),
                ..ContactFields::default()
            },
            ContactFields {
                addresses: Some(vec![Address::default()]),
                ..ContactFields::default()
            },
            ContactFields {
                related: Some(vec![Related::default()]),
                ..ContactFields::default()
            },
        ];
        for f in cases {
            let mut c = card(PERSON);
            let e = apply(&mut c, &f).unwrap_err();
            assert_eq!(e.kind(), "validation", "{e}");
            let msg = e.to_string();
            assert!(
                !msg.contains("zz-invalid") && !msg.contains("07700"),
                "{msg}"
            );
        }
    }

    #[test]
    fn multibyte_text_is_folded_on_character_boundaries() {
        let note = format!("{}{}{}", "é".repeat(40), "€".repeat(20), "𠀋".repeat(10));
        let mut c = card(PERSON);
        apply(
            &mut c,
            &ContactFields {
                note: Some(note.clone()),
                ..ContactFields::default()
            },
        )
        .unwrap();
        let text = c.to_ics();
        let physical: Vec<&str> = text
            .split("\r\n")
            .skip_while(|l| !l.starts_with("NOTE:"))
            .take_while(|l| l.starts_with("NOTE:") || l.starts_with(' '))
            .collect();
        assert!(physical.len() > 2, "{text}");
        assert!(physical.iter().all(|l| l.len() <= 75), "{physical:?}");
        let back = parse_contact(&parse_card(&text).unwrap(), "x");
        assert_eq!(back.note.as_deref(), Some(note.as_str()));
    }

    #[test]
    fn the_size_limit_is_enforced() {
        assert!(check_size("x".repeat(100).as_str(), 100).is_ok());
        let e = check_size("x".repeat(101).as_str(), 100).unwrap_err();
        assert_eq!(e.kind(), "validation");
    }

    #[test]
    fn group_members_are_added_and_removed_by_uid() {
        let mut g = card(GROUP);
        add_members(
            &mut g,
            &[
                "NEW-1".to_string(),
                "5F1C2D3E-4A5B-4C6D-8E7F-901234567890".to_string(),
            ],
        );
        remove_members(
            &mut g,
            &[
                "0A1B2C3D-0000-4000-8000-00000000C0DE".to_string(),
                "DEADBEEF-0000-4000-8000-000000000000".to_string(),
            ],
        );
        rename_group(&mut g, "Reading Group");
        let view = parse_group(&g, "g");
        assert_eq!(view.name, "Reading Group");
        assert_eq!(
            view.members,
            ["5F1C2D3E-4A5B-4C6D-8E7F-901234567890", "NEW-1"]
        );
        let text = g.to_ics();
        assert!(
            text.contains("\r\nX-ADDRESSBOOKSERVER-MEMBER:urn:uuid:NEW-1\r\nREV:"),
            "{text}"
        );
        assert!(
            text.contains("\r\nN:Reading Group\r\nFN:Reading Group\r\n"),
            "{text}"
        );
    }
}
