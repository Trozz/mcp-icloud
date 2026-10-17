//! Labels of multi-valued fields: Apple `X-ABLabel` codes and `TYPE` parameters to plain words,
//! and plain words back to what iCloud and iOS write.

/// The fields that carry labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Email,
    Phone,
    Address,
    Url,
    Date,
    Related,
}

impl Field {
    /// The vCard property (without its `itemN.` group) that holds this field.
    pub fn property(self) -> &'static str {
        match self {
            Field::Email => "EMAIL",
            Field::Phone => "TEL",
            Field::Address => "ADR",
            Field::Url => "URL",
            Field::Date => "X-ABDATE",
            Field::Related => "X-ABRELATEDNAMES",
        }
    }
}

/// How a new or changed value's label is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// `TYPE` parameter values, written as repeated `type=` parameters.
    Types(Vec<&'static str>),
    /// An `itemN.X-ABLabel` value: an Apple code such as `_$!<Mother>!$_`, or free text.
    Label(String),
}

/// `TYPE` values that say nothing a person would call a label.
const IGNORED_TYPES: [&str; 8] = [
    "PREF", "INTERNET", "VOICE", "X400", "POSTAL", "PARCEL", "DOM", "INTL",
];

/// Relation labels written as Apple codes (`mother` -> `_$!<Mother>!$_`).
const RELATIONS: [&str; 17] = [
    "mother",
    "father",
    "parent",
    "brother",
    "sister",
    "sibling",
    "child",
    "son",
    "daughter",
    "spouse",
    "husband",
    "wife",
    "partner",
    "friend",
    "assistant",
    "manager",
    "other",
];

/// `_$!<HomePage>!$_` -> `home page`, `_$!<HomeFAX>!$_` -> `home fax`. Other text is returned as is.
pub fn decode_apple(label: &str) -> String {
    let Some(code) = label
        .strip_prefix("_$!<")
        .and_then(|rest| rest.strip_suffix(">!$_"))
    else {
        return label.to_string();
    };
    let mut out = String::new();
    let mut prev_lower = false;
    for c in code.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase();
        out.extend(c.to_lowercase());
    }
    out
}

/// `mother` -> `_$!<Mother>!$_`; `home page` -> `_$!<HomePage>!$_`.
fn apple_code(words: &str) -> String {
    let camel: String = words
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect();
    format!("_$!<{camel}>!$_")
}

/// The plain label of one value: its `X-ABLabel` when it has one, else its `TYPE` values.
pub fn read_label(field: Field, types: &[String], ab_label: Option<&str>) -> Option<String> {
    if let Some(label) = ab_label.map(str::trim).filter(|l| !l.is_empty()) {
        return Some(decode_apple(label));
    }
    let has = |t: &str| types.iter().any(|x| x.eq_ignore_ascii_case(t));
    if field == Field::Phone {
        if has("IPHONE") {
            return Some("iPhone".into());
        }
        if has("FAX") {
            if has("HOME") {
                return Some("home fax".into());
            }
            if has("WORK") {
                return Some("work fax".into());
            }
            return Some("fax".into());
        }
        if has("CELL") {
            return Some("mobile".into());
        }
        if has("MAIN") {
            return Some("main".into());
        }
        if has("PAGER") {
            return Some("pager".into());
        }
    }
    for t in ["HOME", "WORK", "OTHER"] {
        if has(t) {
            return Some(t.to_ascii_lowercase());
        }
    }
    types
        .iter()
        .find(|t| !IGNORED_TYPES.iter().any(|i| t.eq_ignore_ascii_case(i)))
        .map(|t| t.to_ascii_lowercase())
}

/// How to write a label given by the client (matched case-insensitively).
pub fn write_label(field: Field, label: Option<&str>) -> Written {
    let label = label.map(str::trim).filter(|l| !l.is_empty());
    let key = label.map(str::to_lowercase);
    let custom = || Written::Label(label.unwrap_or_default().to_string());
    match field {
        Field::Email => match key.as_deref() {
            None => Written::Types(vec!["INTERNET"]),
            Some("home") => Written::Types(vec!["INTERNET", "HOME"]),
            Some("work") => Written::Types(vec!["INTERNET", "WORK"]),
            Some("other") => Written::Types(vec!["INTERNET", "OTHER"]),
            Some(_) => custom(),
        },
        Field::Phone => match key.as_deref() {
            None => Written::Types(Vec::new()),
            Some("mobile") => Written::Types(vec!["CELL", "VOICE"]),
            Some("iphone") => Written::Types(vec!["IPHONE", "CELL", "VOICE"]),
            Some("main") => Written::Types(vec!["MAIN"]),
            Some("pager") => Written::Types(vec!["PAGER"]),
            Some("home") => Written::Types(vec!["HOME", "VOICE"]),
            Some("work") => Written::Types(vec!["WORK", "VOICE"]),
            Some("other") => Written::Types(vec!["OTHER", "VOICE"]),
            Some("home fax") => Written::Types(vec!["HOME", "FAX"]),
            Some("work fax") => Written::Types(vec!["WORK", "FAX"]),
            Some(_) => custom(),
        },
        Field::Address => match key.as_deref() {
            None => Written::Types(Vec::new()),
            Some("home") => Written::Types(vec!["HOME"]),
            Some("work") => Written::Types(vec!["WORK"]),
            Some("other") => Written::Types(vec!["OTHER"]),
            Some(_) => custom(),
        },
        Field::Url => match key.as_deref() {
            None => Written::Types(Vec::new()),
            Some("home page") => Written::Label(apple_code("home page")),
            Some("home") => Written::Types(vec!["HOME"]),
            Some("work") => Written::Types(vec!["WORK"]),
            Some("other") => Written::Types(vec!["OTHER"]),
            Some(_) => custom(),
        },
        Field::Date => match key.as_deref() {
            None | Some("other") => Written::Label(apple_code("other")),
            Some("anniversary") => Written::Label(apple_code("anniversary")),
            Some(_) => custom(),
        },
        Field::Related => match key.as_deref() {
            None => Written::Label(apple_code("other")),
            Some(k) if RELATIONS.contains(&k) => Written::Label(apple_code(k)),
            Some(_) => custom(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn apple_codes_become_plain_words() {
        assert_eq!(decode_apple("_$!<HomePage>!$_"), "home page");
        assert_eq!(decode_apple("_$!<Mother>!$_"), "mother");
        assert_eq!(decode_apple("_$!<HomeFAX>!$_"), "home fax");
        assert_eq!(decode_apple("_$!<Anniversary>!$_"), "anniversary");
        assert_eq!(decode_apple("best friend"), "best friend");
        assert_eq!(decode_apple("iPhone"), "iPhone");
    }

    #[test]
    fn an_ab_label_wins_over_types() {
        assert_eq!(
            read_label(Field::Url, &types(&["pref"]), Some("_$!<HomePage>!$_")),
            Some("home page".into())
        );
        assert_eq!(
            read_label(Field::Email, &types(&["INTERNET", "HOME"]), Some("office")),
            Some("office".into())
        );
    }

    #[test]
    fn phone_types_read_like_ios_shows_them() {
        let cases: [(&[&str], &str); 8] = [
            (&["CELL", "VOICE", "pref"], "mobile"),
            (&["cell"], "mobile"),
            (&["IPHONE", "CELL", "VOICE"], "iPhone"),
            (&["HOME", "FAX"], "home fax"),
            (&["WORK", "FAX"], "work fax"),
            (&["MAIN", "pref"], "main"),
            (&["HOME", "VOICE"], "home"),
            (&["PAGER"], "pager"),
        ];
        for (t, want) in cases {
            assert_eq!(
                read_label(Field::Phone, &types(t), None).as_deref(),
                Some(want),
                "{t:?}"
            );
        }
        assert_eq!(
            read_label(Field::Phone, &types(&["VOICE", "pref"]), None),
            None
        );
    }

    #[test]
    fn other_fields_read_home_work_other_and_unknown_types() {
        assert_eq!(
            read_label(Field::Email, &types(&["INTERNET", "WORK", "pref"]), None),
            Some("work".into())
        );
        assert_eq!(read_label(Field::Email, &types(&["INTERNET"]), None), None);
        assert_eq!(
            read_label(Field::Address, &types(&["HOME", "pref"]), None),
            Some("home".into())
        );
        assert_eq!(
            read_label(Field::Url, &types(&["SCHOOL"]), None),
            Some("school".into())
        );
    }

    #[test]
    fn labels_are_written_like_the_spec_table() {
        let t = |v: Vec<&'static str>| Written::Types(v);
        assert_eq!(
            write_label(Field::Email, Some("Home")),
            t(vec!["INTERNET", "HOME"])
        );
        assert_eq!(write_label(Field::Email, None), t(vec!["INTERNET"]));
        assert_eq!(
            write_label(Field::Phone, Some("mobile")),
            t(vec!["CELL", "VOICE"])
        );
        assert_eq!(
            write_label(Field::Phone, Some("iPhone")),
            t(vec!["IPHONE", "CELL", "VOICE"])
        );
        assert_eq!(write_label(Field::Phone, Some("main")), t(vec!["MAIN"]));
        assert_eq!(write_label(Field::Phone, Some("pager")), t(vec!["PAGER"]));
        assert_eq!(
            write_label(Field::Phone, Some("work")),
            t(vec!["WORK", "VOICE"])
        );
        assert_eq!(
            write_label(Field::Phone, Some("home fax")),
            t(vec!["HOME", "FAX"])
        );
        assert_eq!(
            write_label(Field::Phone, Some("work fax")),
            t(vec!["WORK", "FAX"])
        );
        assert_eq!(write_label(Field::Address, Some("other")), t(vec!["OTHER"]));
        assert_eq!(
            write_label(Field::Url, Some("home page")),
            Written::Label("_$!<HomePage>!$_".into())
        );
        assert_eq!(write_label(Field::Url, Some("work")), t(vec!["WORK"]));
        assert_eq!(
            write_label(Field::Date, Some("Anniversary")),
            Written::Label("_$!<Anniversary>!$_".into())
        );
        assert_eq!(
            write_label(Field::Date, None),
            Written::Label("_$!<Other>!$_".into())
        );
        assert_eq!(
            write_label(Field::Related, Some("mother")),
            Written::Label("_$!<Mother>!$_".into())
        );
        assert_eq!(
            write_label(Field::Related, None),
            Written::Label("_$!<Other>!$_".into())
        );
        assert_eq!(
            write_label(Field::Phone, Some("boat")),
            Written::Label("boat".into())
        );
        assert_eq!(
            write_label(Field::Related, Some("best friend")),
            Written::Label("best friend".into())
        );
    }

    #[test]
    fn written_labels_read_back_the_same() {
        for (field, label) in [
            (Field::Email, "home"),
            (Field::Phone, "mobile"),
            (Field::Phone, "iPhone"),
            (Field::Phone, "work fax"),
            (Field::Address, "work"),
            (Field::Url, "home page"),
            (Field::Date, "anniversary"),
            (Field::Related, "mother"),
            (Field::Related, "best friend"),
        ] {
            let back = match write_label(field, Some(label)) {
                Written::Types(t) => {
                    let t: Vec<String> = t.iter().map(|s| s.to_string()).collect();
                    read_label(field, &t, None)
                }
                Written::Label(l) => read_label(field, &[], Some(&l)),
            };
            assert_eq!(back.as_deref(), Some(label), "{field:?}");
        }
    }
}
