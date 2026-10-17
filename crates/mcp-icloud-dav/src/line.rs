//! Lossless content lines for iCalendar (RFC 5545 section 3.1) and vCard (RFC 2426). A parsed
//! property keeps its original text, folds included, and is written back unchanged unless it is
//! edited.

use crate::error::{DavError, Result};

const FOLD_OCTETS: usize = 75;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub values: Vec<String>,
}

/// Where a parsed property came from: its unfolded line and the physical lines (without line
/// terminators) it was folded into.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    line: String,
    physical: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    name: String,
    /// The name as it is written (original case, vCard group included), when known.
    written: Option<String>,
    params: Vec<Param>,
    value: String,
    source: Option<Source>,
}

impl Property {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            written: None,
            params: Vec::new(),
            value: value.into(),
            source: None,
        }
    }

    /// Like `new`, but the name is written exactly as given (for example `item3.X-ABLabel`);
    /// `name()` still returns the upper-case form used for matching.
    pub fn new_cased(name: &str, value: impl Into<String>) -> Self {
        Self {
            written: Some(name.to_string()),
            ..Self::new(name, value)
        }
    }

    /// A property from one unfolded content line, kept exactly as written (name case, parameter
    /// order, case and repetition such as `type=CELL;type=VOICE;type=pref`, and the raw value).
    /// It is written folded at 75 octets on UTF-8 boundaries by the same rule as every edited
    /// property. A line without a name or `:`, with a parameter missing `=`, or containing a line
    /// break is an upstream error, like other parse errors.
    pub fn parse_line(line: &str) -> Result<Self> {
        if line.contains(['\r', '\n']) {
            return Err(DavError::Upstream(format!(
                "invalid content line: {line:?}"
            )));
        }
        Self::parse_folded(line, fold_lines(line))
    }

    pub fn with_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.set_param(name, value);
        self
    }

    /// The upper-case name, vCard group included (`ITEM1.EMAIL`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The vCard group (`item1` for `item1.EMAIL`), as written; `None` without a group.
    pub fn group(&self) -> Option<&str> {
        self.written
            .as_deref()
            .unwrap_or(&self.name)
            .split_once('.')
            .map(|(group, _)| group)
    }

    /// The upper-case name without its vCard group (`EMAIL` for `item1.EMAIL`).
    pub fn base_name(&self) -> &str {
        self.name
            .split_once('.')
            .map_or(self.name.as_str(), |(_, name)| name)
    }

    /// The raw (still escaped) value.
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// First value of the parameter, matched case-insensitively.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .and_then(|p| p.values.first())
            .map(String::as_str)
    }

    pub fn set_param(&mut self, name: &str, value: impl Into<String>) {
        self.source = None;
        let value = value.into();
        match self
            .params
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            Some(p) => p.values = vec![value],
            None => self.params.push(Param {
                name: name.to_ascii_uppercase(),
                values: vec![value],
            }),
        }
    }

    pub fn remove_param(&mut self, name: &str) {
        let before = self.params.len();
        self.params.retain(|p| !p.name.eq_ignore_ascii_case(name));
        if self.params.len() != before {
            self.source = None;
        }
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.source = None;
        self.value = value.into();
    }

    /// The unfolded content line: the original text when unedited.
    pub fn to_line(&self) -> String {
        if let Some(source) = &self.source {
            return source.line.clone();
        }
        let mut line = self.written.clone().unwrap_or_else(|| self.name.clone());
        for p in &self.params {
            line.push(';');
            line.push_str(&p.name);
            line.push('=');
            let values: Vec<String> = p.values.iter().map(|v| quote_param(v)).collect();
            line.push_str(&values.join(","));
        }
        line.push(':');
        line.push_str(&self.value);
        line
    }

    /// Writes the original physical lines when unedited, otherwise the line folded at 75 octets.
    fn write(&self, out: &mut String) {
        match &self.source {
            Some(source) => {
                for physical in &source.physical {
                    out.push_str(physical);
                    out.push_str("\r\n");
                }
            }
            None => push_folded(out, &self.to_line()),
        }
    }

    fn parse_folded(line: &str, physical: Vec<String>) -> Result<Self> {
        let invalid = || DavError::Upstream(format!("invalid content line: {line:?}"));
        let name_end = line.find([';', ':']).ok_or_else(invalid)?;
        let name = &line[..name_end];
        if name.is_empty() {
            return Err(invalid());
        }
        let mut params = Vec::new();
        let mut rest = &line[name_end..];
        while let Some(after) = rest.strip_prefix(';') {
            let eq = after.find('=').ok_or_else(invalid)?;
            let pname = &after[..eq];
            let mut values = Vec::new();
            let mut s = &after[eq + 1..];
            loop {
                let (v, tail) = if let Some(q) = s.strip_prefix('"') {
                    let close = q.find('"').ok_or_else(invalid)?;
                    (&q[..close], &q[close + 1..])
                } else {
                    let end = s.find([',', ';', ':']).ok_or_else(invalid)?;
                    (&s[..end], &s[end..])
                };
                values.push(v.to_string());
                match tail.strip_prefix(',') {
                    Some(t) => s = t,
                    None => {
                        s = tail;
                        break;
                    }
                }
            }
            params.push(Param {
                name: pname.to_ascii_uppercase(),
                values,
            });
            rest = s;
        }
        let value = rest.strip_prefix(':').ok_or_else(invalid)?;
        Ok(Self {
            name: name.to_ascii_uppercase(),
            written: Some(name.to_string()),
            params,
            value: value.to_string(),
            source: Some(Source {
                line: line.to_string(),
                physical,
            }),
        })
    }
}

fn quote_param(v: &str) -> String {
    let clean: String = v.chars().filter(|c| *c != '"').collect();
    if clean.contains([':', ';', ',']) {
        return format!("\"{clean}\"");
    }
    clean
}

fn invalid_data(detail: impl std::fmt::Display) -> DavError {
    DavError::Upstream(format!("invalid data: {detail}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub properties: Vec<Property>,
    pub components: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            properties: Vec::new(),
            components: Vec::new(),
        }
    }

    /// Parses one top-level component (VCALENDAR or VCARD). Accepts CRLF or LF line endings.
    pub fn parse(text: &str) -> Result<Self> {
        let mut stack: Vec<Component> = Vec::new();
        let mut done: Option<Component> = None;
        for (line, physical) in unfold_physical(text) {
            if line.is_empty() {
                continue;
            }
            if done.is_some() {
                return Err(invalid_data(
                    "content after the end of the top-level component",
                ));
            }
            let prop = Property::parse_folded(&line, physical)?;
            match prop.name.as_str() {
                "BEGIN" => stack.push(Component::new(prop.value.trim())),
                "END" => {
                    let comp = stack
                        .pop()
                        .ok_or_else(|| invalid_data(format!("unexpected {line:?}")))?;
                    if !comp.name.eq_ignore_ascii_case(prop.value.trim()) {
                        return Err(invalid_data(format!("{line:?} closes {}", comp.name)));
                    }
                    match stack.last_mut() {
                        Some(parent) => parent.components.push(comp),
                        None => done = Some(comp),
                    }
                }
                _ => match stack.last_mut() {
                    Some(c) => c.properties.push(prop),
                    None => return Err(invalid_data(format!("{line:?} outside a component"))),
                },
            }
        }
        if !stack.is_empty() {
            return Err(invalid_data("unterminated component"));
        }
        done.ok_or_else(|| invalid_data("empty"))
    }

    /// Serializes any component tree (iCalendar or vCard) with CRLF line endings. Unedited
    /// properties keep their original folds; everything else is folded at 75 octets.
    pub fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        push_folded(out, &format!("BEGIN:{}", self.name));
        for p in &self.properties {
            p.write(out);
        }
        for c in &self.components {
            c.write(out);
        }
        push_folded(out, &format!("END:{}", self.name));
    }

    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn prop_mut(&mut self, name: &str) -> Option<&mut Property> {
        self.properties
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn props<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.properties
            .iter()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    /// Raw value of the first property with this name.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.prop(name).map(Property::value)
    }

    /// Replaces every property with this name by `prop`, keeping the position of the first one.
    pub fn set(&mut self, prop: Property) {
        match self
            .properties
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(&prop.name))
        {
            Some(i) => {
                let name = prop.name.clone();
                self.properties[i] = prop;
                let mut index = 0;
                self.properties.retain(|p| {
                    let keep = index == i || !p.name.eq_ignore_ascii_case(&name);
                    index += 1;
                    keep
                });
            }
            None => self.properties.push(prop),
        }
    }

    pub fn push(&mut self, prop: Property) {
        self.properties.push(prop);
    }

    pub fn remove(&mut self, name: &str) {
        self.properties
            .retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.components
            .iter()
            .filter(move |c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn children_mut<'a>(
        &'a mut self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a mut Component> + 'a {
        self.components
            .iter_mut()
            .filter(move |c| c.name.eq_ignore_ascii_case(name))
    }
}

/// Logical lines with the physical lines (without terminators) each was folded from.
fn unfold_physical(text: &str) -> Vec<(String, Vec<String>)> {
    let mut lines: Vec<(String, Vec<String>)> = Vec::new();
    for physical in text.split('\n') {
        let physical = physical.strip_suffix('\r').unwrap_or(physical);
        if let Some(cont) = physical
            .strip_prefix(' ')
            .or_else(|| physical.strip_prefix('\t'))
            && let Some((last, parts)) = lines.last_mut()
        {
            last.push_str(cont);
            parts.push(physical.to_string());
            continue;
        }
        lines.push((physical.to_string(), vec![physical.to_string()]));
    }
    while lines.last().is_some_and(|(line, _)| line.is_empty()) {
        lines.pop();
    }
    lines
}

/// Logical lines: continuation lines (leading space or tab) are joined to the previous line.
pub fn unfold(text: &str) -> Vec<String> {
    unfold_physical(text)
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

/// The physical lines of `line` folded at 75 octets on UTF-8 boundaries; continuation lines
/// start with a space. This is the only fold rule.
fn fold_lines(line: &str) -> Vec<String> {
    let mut physical = Vec::new();
    let mut limit = FOLD_OCTETS;
    let mut prefix = "";
    let mut rest = line;
    loop {
        if rest.len() <= limit {
            physical.push(format!("{prefix}{rest}"));
            return physical;
        }
        let mut cut = limit;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        physical.push(format!("{prefix}{}", &rest[..cut]));
        rest = &rest[cut..];
        prefix = " ";
        limit = FOLD_OCTETS - 1;
    }
}

fn push_folded(out: &mut String, line: &str) {
    for physical in fold_lines(line) {
        out.push_str(&physical);
        out.push_str("\r\n");
    }
}

/// Escapes TEXT values (RFC 5545 section 3.3.11, RFC 2426 section 4).
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Folds the way Apple does: 75 octets on the first line, then a space and 75 more octets
    /// on each continuation line (ours put 74 after the space).
    fn apple_fold(line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        while rest.len() > 75 {
            out.push_str(&rest[..75]);
            out.push_str("\r\n ");
            rest = &rest[75..];
        }
        out.push_str(rest);
        out.push_str("\r\n");
        out
    }

    /// A synthetic contact shaped like one the iPhone writes: a photo URL whose fold falls inside
    /// the X-ABCROP-RECTANGLE parameter, an opaque blob and Apple item groups.
    fn apple_card() -> String {
        let grammar = format!("item1.X-ADDRESSING-GRAMMAR:{}", "QUJD".repeat(100));
        [
            "BEGIN:VCARD",
            "VERSION:3.0",
            "PRODID:-//Apple Inc.//iOS 27.2//EN",
            "N:Probe;Example;;;",
            "FN:Example Probe",
            grammar.as_str(),
            "EMAIL;type=INTERNET;type=HOME;type=pref:user@example.com",
            "TEL;type=CELL;type=VOICE;type=pref:+44 7700 900123",
            "item2.URL;type=pref:https://example.com",
            "item2.X-ABLabel:_$!<HomePage>!$_",
            "NOTE:Synthetic card\\, used in tests",
            "PHOTO;TYPE=JPEG;X-ABCROP-RECTANGLE=ABClipRect_1&0&359&2151&2151&AAECAwQFBgcICQoLDA0ODw==;VALUE=uri:https://gateway.icloud.com/contacts/1234567890/ck/card/0123456789abcdef0123456789abcdef",
            "X-IMAGETYPE:PHOTO",
            "X-IMAGEHASH:AAECAwQFBgcICQoLDA0ODw==",
            "REV:2026-01-01T00:00:00Z",
            "UID:00000000-0000-4000-8000-000000000001",
            "END:VCARD",
        ]
        .iter()
        .map(|line| apple_fold(line))
        .collect()
    }

    #[test]
    fn apple_folded_cards_round_trip_byte_for_byte() {
        let text = apple_card();
        assert!(
            text.split("\r\n").any(|l| l.len() == 76),
            "the fixture must use Apple's 75-octet continuation lines"
        );
        assert!(text.contains("&AAECAwQFBgc\r\n ICQoLDA0ODw==;VALUE=uri:"));
        let card = Component::parse(&text).unwrap();
        assert_eq!(card.to_ics(), text);
    }

    #[test]
    fn an_edit_refolds_only_the_edited_property() {
        let text = apple_card();
        let mut card = Component::parse(&text).unwrap();
        card.prop_mut("NOTE").unwrap().set_value("Edited");
        let edited = card.to_ics();
        let keep = |s: &str| -> Vec<String> {
            s.split("\r\n")
                .filter(|l| !l.starts_with("NOTE:"))
                .map(String::from)
                .collect()
        };
        assert_eq!(keep(&edited), keep(&text));
        assert!(edited.contains("\r\nNOTE:Edited\r\n"));

        let value = card.prop("PHOTO").unwrap().value().to_string();
        card.prop_mut("PHOTO").unwrap().set_value(value);
        let refolded = card.to_ics();
        assert_ne!(refolded, edited, "an edited property is folded again");
        assert_eq!(unfold(&refolded), unfold(&edited));
        let photo: Vec<&str> = refolded
            .split("\r\n")
            .skip_while(|l| !l.starts_with("PHOTO;"))
            .take_while(|l| l.starts_with("PHOTO;") || l.starts_with(' '))
            .collect();
        assert!(photo.len() > 1, "{photo:?}");
        assert!(photo.iter().all(|l| l.len() <= 75), "{photo:?}");
    }

    #[test]
    fn lf_input_is_written_with_crlf_and_keeps_its_folds() {
        let card = Component::parse("BEGIN:VCARD\nNOTE:abc\n def\nEND:VCARD\n").unwrap();
        assert_eq!(card.value("NOTE"), Some("abcdef"));
        assert_eq!(
            card.to_ics(),
            "BEGIN:VCARD\r\nNOTE:abc\r\n def\r\nEND:VCARD\r\n"
        );
    }

    #[test]
    fn vcard_groups_and_base_names() {
        let p = Property::parse_line("item1.EMAIL;type=INTERNET:user@example.com").unwrap();
        assert_eq!(p.name(), "ITEM1.EMAIL");
        assert_eq!(p.group(), Some("item1"));
        assert_eq!(p.base_name(), "EMAIL");
        assert_eq!(p.param("TYPE"), Some("INTERNET"));
        let plain = Property::parse_line("EMAIL:user@example.com").unwrap();
        assert_eq!(plain.group(), None);
        assert_eq!(plain.base_name(), "EMAIL");
        let made = Property::new("item4.TEL", "+44 7700 900123");
        assert_eq!(made.group(), Some("ITEM4"));
        assert_eq!(made.base_name(), "TEL");
    }

    #[test]
    fn new_cased_writes_the_name_as_given() {
        let p = Property::new_cased("item3.X-ABLabel", "_$!<HomePage>!$_");
        assert_eq!(p.name(), "ITEM3.X-ABLABEL");
        assert_eq!(p.group(), Some("item3"));
        assert_eq!(p.base_name(), "X-ABLABEL");
        assert_eq!(p.to_line(), "item3.X-ABLabel:_$!<HomePage>!$_");
        let mut c = Component::new("VCARD");
        c.push(p);
        assert_eq!(
            c.to_ics(),
            "BEGIN:VCARD\r\nitem3.X-ABLabel:_$!<HomePage>!$_\r\nEND:VCARD\r\n"
        );
        assert!(c.prop("item3.x-ablabel").is_some());
    }

    #[test]
    fn edited_properties_keep_the_name_as_written() {
        let mut p = Property::parse_line("item2.X-ABLabel:old").unwrap();
        p.set_value("new");
        assert_eq!(p.to_line(), "item2.X-ABLabel:new");
    }

    #[test]
    fn parsed_lines_keep_repeated_params_exactly() {
        let tel = "TEL;type=CELL;type=VOICE;type=pref:+44 7700 900123";
        let p = Property::parse_line(tel).unwrap();
        assert_eq!(p.name(), "TEL");
        assert_eq!(p.params().len(), 3);
        assert_eq!(p.param("TYPE"), Some("CELL"));
        assert_eq!(p.to_line(), tel);
        let mut c = Component::new("VCARD");
        c.push(Property::parse_line("item1.X-ABLabel:_$!<HomePage>!$_").unwrap());
        c.push(p);
        assert_eq!(
            c.to_ics(),
            format!("BEGIN:VCARD\r\nitem1.X-ABLabel:_$!<HomePage>!$_\r\n{tel}\r\nEND:VCARD\r\n")
        );
    }

    #[test]
    fn parse_line_folds_long_multibyte_lines_on_char_boundaries() {
        let line = format!("NOTE:{}", "Caf\u{e9} \u{bd} \u{20ac} \u{1d11e} ".repeat(12));
        assert!(line.len() > 150);
        let mut c = Component::new("VCARD");
        c.push(Property::parse_line(&line).unwrap());
        let text = c.to_ics();
        let physical: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert!(physical.iter().all(|l| l.len() <= 75), "{physical:?}");
        assert!(physical.iter().filter(|l| l.starts_with(' ')).count() >= 2);
        let note: Vec<&str> = physical
            .iter()
            .copied()
            .filter(|l| l.starts_with("NOTE:") || l.starts_with(' '))
            .collect();
        assert!(
            note[..note.len() - 1].iter().all(|l| l.len() < 75),
            "each fold steps back from a multi-byte character: {note:?}"
        );
        assert_eq!(unfold(&text), ["BEGIN:VCARD", line.as_str(), "END:VCARD"]);
        let mut edited = Component::new("VCARD");
        edited.push(Property::new("NOTE", &line["NOTE:".len()..]));
        assert_eq!(
            edited.to_ics(),
            text,
            "parse_line and edited properties share one fold rule"
        );
    }

    #[test]
    fn invalid_lines_are_upstream_errors() {
        for bad in [
            "",
            "no colon here",
            ":value only",
            "NOTE;type:x",
            "NOTE:a\r\nb",
            "NOTE:a\nb",
        ] {
            let e = Property::parse_line(bad).unwrap_err();
            assert!(matches!(e, DavError::Upstream(_)), "{bad:?}: {e:?}");
        }
    }

    #[test]
    fn rebuilt_lines_quote_param_values_that_need_it() {
        let p = Property::new("attendee", "mailto:a@example.com")
            .with_param("CN", "Doe, Jane")
            .with_param("PARTSTAT", "ACCEPTED");
        assert_eq!(
            p.to_line(),
            "ATTENDEE;CN=\"Doe, Jane\";PARTSTAT=ACCEPTED:mailto:a@example.com"
        );
        let mut q = Property::parse_line("DTSTART;TZID=Europe/London:20260101T090000").unwrap();
        q.set_param("tzid", "America/New_York");
        assert_eq!(q.to_line(), "DTSTART;TZID=America/New_York:20260101T090000");
        q.remove_param("TZID");
        assert_eq!(q.to_line(), "DTSTART:20260101T090000");
    }

    #[test]
    fn long_lines_fold_at_75_octets_on_char_boundaries() {
        let mut c = Component::new("VEVENT");
        c.push(Property::new("SUMMARY", "é".repeat(100)));
        let ics = c.to_ics();
        for physical in ics.split("\r\n") {
            assert!(physical.len() <= 75, "{} octets", physical.len());
        }
        let back = Component::parse(&ics).unwrap();
        assert_eq!(back.value("SUMMARY").unwrap(), "é".repeat(100));
    }

    #[test]
    fn set_replaces_all_of_a_name_in_place() {
        let mut c = Component::new("VEVENT");
        c.push(Property::new("UID", "1"));
        c.push(Property::new("EXDATE", "a"));
        c.push(Property::new("SUMMARY", "x"));
        c.push(Property::new("EXDATE", "b"));
        c.set(Property::new("EXDATE", "c"));
        let names: Vec<_> = c
            .properties
            .iter()
            .map(|p| format!("{}:{}", p.name(), p.value()))
            .collect();
        assert_eq!(names, ["UID:1", "EXDATE:c", "SUMMARY:x"]);
        c.set(Property::new("LOCATION", "here"));
        assert_eq!(c.properties.last().unwrap().name(), "LOCATION");
        c.remove("EXDATE");
        assert!(c.prop("EXDATE").is_none());
    }

    #[test]
    fn text_escaping_round_trips() {
        let s = "a, b; c\\d\nnext";
        assert_eq!(escape_text(s), "a\\, b\\; c\\\\d\\nnext");
        assert_eq!(unescape_text(&escape_text(s)), s);
        assert_eq!(unescape_text("x\\Ny"), "x\ny");
    }

    #[test]
    fn malformed_data_is_an_upstream_error() {
        for bad in [
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nEND:VCALENDAR\n",
            "BEGIN:VCALENDAR\n",
            "SUMMARY:x\n",
            "BEGIN:VCARD\nno colon here\nEND:VCARD\n",
            "BEGIN:VCARD\nEND:VCARD\nNOTE:after\n",
            "",
        ] {
            let e = Component::parse(bad).unwrap_err();
            assert!(matches!(e, DavError::Upstream(_)), "{bad:?}: {e:?}");
        }
    }
}
