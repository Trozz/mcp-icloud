//! The repository is public: test data must be synthetic. This scans every file under `tests/`
//! (fixtures and test code). It allows only the example email domains, the example principal id
//! and phone numbers from the ranges reserved for fiction; the scanners' own negative examples are
//! built at run time so this file passes its own scan.

use std::path::{Path, PathBuf};

const ALLOWED_DOMAINS: [&str; 3] = ["example.com", "example.org", "example.net"];
const EXAMPLE_PRINCIPAL: &str = "1234567890";

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn test_files() -> Vec<(String, String)> {
    let mut paths = Vec::new();
    files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"),
        &mut paths,
    );
    let out: Vec<(String, String)> = paths
        .into_iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            Some((p.display().to_string(), text))
        })
        .collect();
    assert!(out.len() > 5, "too few test files found");
    out
}

fn email_domains(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut domains = Vec::new();
    for (i, b) in bytes.iter().enumerate() {
        if *b != b'@' || i == 0 || !is_local_char(bytes[i - 1]) {
            continue;
        }
        let domain: String = text[i + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        if domain.contains('.') {
            domains.push(domain);
        }
    }
    domains
}

fn is_local_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"._%+-".contains(&b)
}

fn principal_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for marker in ["/", "urn:uuid:"] {
        for (start, _) in text.match_indices(marker) {
            let digits: String = text[start + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let next = text[start + marker.len() + digits.len()..].chars().next();
            let terminated = marker != "/" || next == Some('/');
            if digits.len() >= 6 && terminated {
                ids.push(digits);
            }
        }
    }
    ids
}

/// The values of `TEL` content lines, also inside Rust string literals.
fn tel_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("TEL") {
        let before = text[..i].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let rest = &text[i + 3..];
        if !rest.starts_with([';', ':']) {
            continue;
        }
        let Some(colon) = rest.find(':') else {
            continue;
        };
        if rest[..colon].contains(['\n', '"', '\\']) {
            continue;
        }
        let value: String = rest[colon + 1..]
            .chars()
            .take_while(|c| !matches!(c, '\r' | '\n' | '\\' | '"' | '<'))
            .collect();
        out.push(value);
    }
    out
}

/// +44 7700 900xxx or 07700 900xxx (UK drama numbers), or 555-01xx (North American fiction).
fn is_fiction(value: &str) -> bool {
    let d: String = value.chars().filter(char::is_ascii_digit).collect();
    let uk = ["447700900", "07700900"]
        .iter()
        .any(|p| d.starts_with(p) && d.len() == p.len() + 3);
    let na = d.len() >= 7 && d[d.len() - 7..].starts_with("55501");
    uk || na
}

#[test]
fn test_files_only_use_example_email_domains() {
    for (path, text) in test_files() {
        for domain in email_domains(&text) {
            assert!(
                ALLOWED_DOMAINS
                    .iter()
                    .any(|d| domain == *d || domain.ends_with(&format!(".{d}"))),
                "{path} contains an email address at {domain}; use example.com, example.org or example.net"
            );
        }
    }
}

#[test]
fn test_files_only_use_the_example_principal_id() {
    for (path, text) in test_files() {
        for id in principal_ids(&text) {
            assert_eq!(
                id, EXAMPLE_PRINCIPAL,
                "{path} contains principal id {id}; use {EXAMPLE_PRINCIPAL}"
            );
        }
    }
}

#[test]
fn test_files_only_use_fictional_phone_numbers() {
    for (path, text) in test_files() {
        for value in tel_values(&text) {
            assert!(
                is_fiction(&value),
                "{path} has a phone number outside the fiction ranges: {value}"
            );
        }
    }
}

#[test]
fn the_scanners_catch_real_looking_data() {
    let at = "@";
    assert_eq!(
        email_domains(&format!("mailto:someone{at}mail.test.invalid x")),
        ["mail.test.invalid"]
    );
    assert_eq!(
        principal_ids(&format!("<href>/{}/principal/</href>", 987_654_321)),
        ["987654321"]
    );
    let name = ["T", "EL"].concat();
    assert_eq!(
        tel_values(&format!("{name};type=CELL:+44 20 7123 4567\r\n")),
        ["+44 20 7123 4567"]
    );
    assert!(!is_fiction("+44 20 7123 4567"));
    assert!(is_fiction("+44 7700 900123"));
    assert!(is_fiction("07700 900123"));
    assert!(is_fiction("+1 202-555-0123"));
}
