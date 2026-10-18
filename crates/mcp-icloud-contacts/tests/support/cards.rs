//! Synthetic cards for tests. Every name, number and address is made up.
use super::fake_carddav::FakeState;

pub const PERSON: &str = include_str!("../fixtures/apple-person.vcf");
pub const COMPANY: &str = include_str!("../fixtures/apple-company.vcf");
pub const GROUP: &str = include_str!("../fixtures/apple-group.vcf");
pub const PERSON_UID: &str = "5F1C2D3E-4A5B-4C6D-8E7F-901234567890";
pub const COMPANY_UID: &str = "0A1B2C3D-0000-4000-8000-00000000C0DE";
pub const GROUP_UID: &str = "9B8A7C6D-1111-4222-8333-444455556666";
/// The person's file name is not its UID, as for many real iCloud cards.
pub const PERSON_FILE: &str = "B3F0F1A2-7777-4888-9999-AAAABBBBCCCC.vcf";
pub const COMPANY_FILE: &str = "0A1B2C3D-0000-4000-8000-00000000C0DE.vcf";
pub const GROUP_FILE: &str = "9B8A7C6D-1111-4222-8333-444455556666.vcf";

/// A small contact card with CRLF line endings.
pub fn contact(uid: &str, given: &str, family: &str, extra: &[&str]) -> String {
    let mut lines = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:3.0".into(),
        "PRODID:-//Apple Inc.//iOS 27.2//EN".into(),
        format!("N:{family};{given};;;"),
        format!("FN:{given} {family}"),
    ];
    lines.extend(extra.iter().map(|l| l.to_string()));
    lines.push("REV:2026-01-01T00:00:00Z".into());
    lines.push(format!("UID:{uid}"));
    lines.push("END:VCARD".into());
    lines.join("\r\n") + "\r\n"
}

/// A group card listing these member UIDs.
pub fn group(uid: &str, name: &str, members: &[&str]) -> String {
    let mut lines = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:3.0".into(),
        "PRODID:-//Apple Inc.//iOS 27.2//EN".into(),
        format!("N:{name}"),
        format!("FN:{name}"),
        "X-ADDRESSBOOKSERVER-KIND:group".into(),
    ];
    lines.extend(
        members
            .iter()
            .map(|m| format!("X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:{m}")),
    );
    lines.push(format!("UID:{uid}"));
    lines.push("END:VCARD".into());
    lines.join("\r\n") + "\r\n"
}

/// The usual account: Alex (the owner's own card), a company, and a group of both plus one
/// member UID that no longer exists.
pub fn seed(state: &mut FakeState) {
    state.put(PERSON_FILE, PERSON);
    state.put(COMPANY_FILE, COMPANY);
    state.put(GROUP_FILE, GROUP);
    state.me = Some(PERSON_FILE.into());
}
