//! Copying a contact's photo onto another contact. The photo lines are copied exactly as stored
//! (original folds included), so the copy points at the same iCloud photo.

use mcp_icloud_dav::line::{Component, Property};

/// The properties that make up an iCloud contact photo.
pub const PHOTO_PROPERTIES: [&str; 4] = [
    "PHOTO",
    "X-IMAGETYPE",
    "X-IMAGEHASH",
    "X-SHARED-PHOTO-DISPLAY-PREF",
];

fn is_photo_line(p: &Property) -> bool {
    PHOTO_PROPERTIES
        .iter()
        .any(|n| p.base_name().eq_ignore_ascii_case(n))
}

/// Replaces `to`'s photo lines with copies of `from`'s, where `to`'s first photo line was (else
/// at the end). false, and `to` unchanged, when `from` has no `PHOTO`.
pub fn copy_photo(from: &Component, to: &mut Component) -> bool {
    let lines: Vec<Property> = from
        .properties
        .iter()
        .filter(|p| is_photo_line(p))
        .cloned()
        .collect();
    if !lines.iter().any(|p| p.base_name() == "PHOTO") {
        return false;
    }
    let at = to.properties.iter().position(is_photo_line);
    to.properties.retain(|p| !is_photo_line(p));
    let at = at.unwrap_or(to.properties.len()).min(to.properties.len());
    to.properties.splice(at..at, lines);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::parse::parse_card;

    const PERSON: &str = include_str!("../../tests/fixtures/apple-person.vcf");
    const COMPANY: &str = include_str!("../../tests/fixtures/apple-company.vcf");

    fn photo_lines(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut inside = false;
        for line in text.replace("\r\n", "\n").split('\n') {
            if line.starts_with(' ') {
                if inside {
                    out.push(line.to_string());
                }
                continue;
            }
            inside = PHOTO_PROPERTIES
                .iter()
                .any(|n| line.starts_with(&format!("{n}:")) || line.starts_with(&format!("{n};")));
            if inside {
                out.push(line.to_string());
            }
        }
        out
    }

    #[test]
    fn the_photo_lines_are_copied_byte_for_byte() {
        let from = parse_card(PERSON).unwrap();
        let mut to = parse_card(COMPANY).unwrap();
        assert!(copy_photo(&from, &mut to));
        let written = to.to_ics();
        assert_eq!(photo_lines(&written), photo_lines(PERSON));
        assert_eq!(photo_lines(PERSON).len(), 6, "PHOTO has 3 physical lines");
        let mut rest: Vec<String> = written
            .split("\r\n")
            .filter(|l| !photo_lines(&written).contains(&l.to_string()))
            .map(str::to_string)
            .collect();
        rest.retain(|l| !l.is_empty());
        let company: Vec<String> = COMPANY.lines().map(str::to_string).collect();
        assert_eq!(rest, company, "every other line is unchanged");
    }

    #[test]
    fn an_old_photo_is_replaced_in_place() {
        let mut from = parse_card(PERSON).unwrap();
        let mut to = parse_card(PERSON).unwrap();
        from.properties
            .retain(|p| p.base_name() != "X-SHARED-PHOTO-DISPLAY-PREF");
        let photo_at = to
            .properties
            .iter()
            .position(|p| p.base_name() == "PHOTO")
            .unwrap();
        assert!(copy_photo(&from, &mut to));
        assert_eq!(to.properties[photo_at].base_name(), "PHOTO");
        let names: Vec<&str> = to.properties.iter().map(|p| p.base_name()).collect();
        assert!(!names.contains(&"X-SHARED-PHOTO-DISPLAY-PREF"), "{names:?}");
        assert_eq!(names.iter().filter(|n| **n == "PHOTO").count(), 1);
    }

    #[test]
    fn a_card_without_a_photo_copies_nothing() {
        let from = parse_card(COMPANY).unwrap();
        let mut to = parse_card(PERSON).unwrap();
        let before = to.clone();
        assert!(!copy_photo(&from, &mut to));
        assert_eq!(to, before);
    }
}
