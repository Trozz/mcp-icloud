//! The repository is public: fixtures must be synthetic. This test names no real data; it only
//! allows the example domains and the example principal id.

use std::path::Path;

const ALLOWED_DOMAINS: [&str; 3] = ["example.com", "example.org", "example.net"];
const EXAMPLE_PRINCIPAL: &str = "1234567890";

fn fixture_files() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let text = std::fs::read_to_string(&path).unwrap();
            out.push((path.display().to_string(), text));
        }
    }
    assert!(!out.is_empty(), "no fixtures found in {}", dir.display());
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

#[test]
fn fixtures_only_use_example_email_domains() {
    for (path, text) in fixture_files() {
        for domain in email_domains(&text) {
            assert!(
                ALLOWED_DOMAINS
                    .iter()
                    .any(|d| domain == *d || domain.ends_with(&format!(".{d}"))),
                "{path} contains an email address at {domain}; fixtures must use example.com, example.org or example.net"
            );
        }
    }
}

#[test]
fn fixtures_only_use_the_example_principal_id() {
    for (path, text) in fixture_files() {
        for id in principal_ids(&text) {
            assert_eq!(
                id, EXAMPLE_PRINCIPAL,
                "{path} contains principal id {id}; use {EXAMPLE_PRINCIPAL}"
            );
        }
    }
}

#[test]
fn the_scanners_catch_real_looking_data() {
    assert_eq!(
        email_domains("mailto:someone@mail.test.invalid x"),
        ["mail.test.invalid"]
    );
    assert_eq!(
        principal_ids("<href>/987654321/principal/</href>"),
        ["987654321"]
    );
    assert_eq!(principal_ids("urn:uuid:987654321"), ["987654321"]);
    assert!(principal_ids("DTSTART:20260101T090000").is_empty());
}
