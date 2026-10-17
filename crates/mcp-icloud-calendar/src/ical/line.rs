//! Lossless iCalendar content lines (RFC 5545 section 3.1). A parsed property keeps its original
//! unfolded line and is written back unchanged unless it is edited.

use crate::error::{CalError, Result};

const FOLD_OCTETS: usize = 75;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    name: String,
    params: Vec<Param>,
    value: String,
    raw: Option<String>,
}

impl Property {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.into(),
            raw: None,
        }
    }

    pub fn with_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.set_param(name, value);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
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
        self.raw = None;
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
            self.raw = None;
        }
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.raw = None;
        self.value = value.into();
    }

    /// The unfolded content line: the original text when unedited.
    pub fn to_line(&self) -> String {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut line = self.name.clone();
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

    fn parse(line: &str) -> Result<Self> {
        let invalid = || CalError::Upstream(format!("invalid iCalendar line: {line:?}"));
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
            params,
            value: value.to_string(),
            raw: Some(line.to_string()),
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

    /// Parses one top-level component (normally VCALENDAR). Accepts CRLF or LF line endings.
    pub fn parse(text: &str) -> Result<Self> {
        let mut stack: Vec<Component> = Vec::new();
        let mut done: Option<Component> = None;
        for line in unfold(text) {
            if line.is_empty() {
                continue;
            }
            if done.is_some() {
                return Err(CalError::Upstream(
                    "invalid iCalendar data: content after the end of the calendar".into(),
                ));
            }
            let prop = Property::parse(&line)?;
            match prop.name.as_str() {
                "BEGIN" => stack.push(Component::new(prop.value.trim())),
                "END" => {
                    let comp = stack.pop().ok_or_else(|| {
                        CalError::Upstream(format!("invalid iCalendar data: unexpected {line:?}"))
                    })?;
                    if !comp.name.eq_ignore_ascii_case(prop.value.trim()) {
                        return Err(CalError::Upstream(format!(
                            "invalid iCalendar data: {line:?} closes {}",
                            comp.name
                        )));
                    }
                    match stack.last_mut() {
                        Some(parent) => parent.components.push(comp),
                        None => done = Some(comp),
                    }
                }
                _ => match stack.last_mut() {
                    Some(c) => c.properties.push(prop),
                    None => {
                        return Err(CalError::Upstream(format!(
                            "invalid iCalendar data: {line:?} outside a component"
                        )));
                    }
                },
            }
        }
        if !stack.is_empty() {
            return Err(CalError::Upstream(
                "invalid iCalendar data: unterminated component".into(),
            ));
        }
        done.ok_or_else(|| CalError::Upstream("invalid iCalendar data: empty".into()))
    }

    /// Serializes with CRLF line endings, folded at 75 octets.
    pub fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        push_folded(out, &format!("BEGIN:{}", self.name));
        for p in &self.properties {
            push_folded(out, &p.to_line());
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

/// Logical lines: continuation lines (leading space or tab) are joined to the previous line.
pub fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for physical in text.split('\n') {
        let physical = physical.strip_suffix('\r').unwrap_or(physical);
        if let Some(cont) = physical
            .strip_prefix(' ')
            .or_else(|| physical.strip_prefix('\t'))
            && let Some(last) = lines.last_mut()
        {
            last.push_str(cont);
            continue;
        }
        lines.push(physical.to_string());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn push_folded(out: &mut String, line: &str) {
    let mut limit = FOLD_OCTETS;
    let mut rest = line;
    loop {
        if rest.len() <= limit {
            out.push_str(rest);
            out.push_str("\r\n");
            return;
        }
        let mut cut = limit;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push_str(&rest[..cut]);
        out.push_str("\r\n ");
        rest = &rest[cut..];
        limit = FOLD_OCTETS - 1;
    }
}

/// Escapes TEXT values (RFC 5545 section 3.3.11).
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

    const APPLE: &str = include_str!("../../tests/fixtures/apple-allday.ics");
    const GOOGLE: &str = include_str!("../../tests/fixtures/google-invite.ics");
    const OUTLOOK: &str = include_str!("../../tests/fixtures/outlook-invite.ics");
    const WEEKLY: &str = include_str!("../../tests/fixtures/apple-weekly.ics");
    const OVERRIDE: &str = include_str!("../../tests/fixtures/master-override.ics");

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

    #[test]
    fn rebuilt_lines_quote_param_values_that_need_it() {
        let p = Property::new("attendee", "mailto:a@example.com")
            .with_param("CN", "Doe, Jane")
            .with_param("PARTSTAT", "ACCEPTED");
        assert_eq!(
            p.to_line(),
            "ATTENDEE;CN=\"Doe, Jane\";PARTSTAT=ACCEPTED:mailto:a@example.com"
        );
        let mut q = Property::parse("DTSTART;TZID=Europe/London:20260101T090000").unwrap();
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
            "BEGIN:VCALENDAR\nno colon here\nEND:VCALENDAR\n",
            "",
        ] {
            let e = Component::parse(bad).unwrap_err();
            assert_eq!(e.kind(), "upstream", "{bad:?}");
        }
    }
}
