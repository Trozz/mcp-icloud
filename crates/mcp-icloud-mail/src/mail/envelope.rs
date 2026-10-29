use super::parse::parse;
use crate::error::{MailError, Result};
use mail_parser::{HeaderName, HeaderValue};

fn header_end(raw: &[u8]) -> usize {
    if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        return i + 2;
    }
    if let Some(i) = raw.windows(2).position(|w| w == b"\n\n") {
        return i + 1;
    }
    raw.len()
}

/// True when `line` starts header `name`: the name, optional spaces or tabs, then a colon.
fn is_header(line: &[u8], name: &str) -> bool {
    if line.len() <= name.len() || !line[..name.len()].eq_ignore_ascii_case(name.as_bytes()) {
        return false;
    }
    line[name.len()..]
        .iter()
        .find(|&&b| b != b' ' && b != b'\t')
        == Some(&b':')
}

/// Removes every occurrence of a header (including folded continuation lines) from the
/// header section only. The body is never touched.
pub fn strip_header(raw: &[u8], name: &str) -> Vec<u8> {
    let end = header_end(raw);
    let (head, rest) = raw.split_at(end);
    let mut out = Vec::with_capacity(raw.len());
    let mut skipping = false;
    for line in head.split_inclusive(|&b| b == b'\n') {
        let continuation = matches!(line.first(), Some(b' ' | b'\t'));
        if !continuation {
            skipping = is_header(line, name);
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(rest);
    out
}

/// The bytes handed to SMTP: every Bcc header removed, then re-parsed to confirm that the parser
/// that chose the envelope recipients sees no Bcc header either.
pub fn strip_bcc(raw: &[u8]) -> Result<Vec<u8>> {
    let out = strip_header(raw, "Bcc");
    if parse(&out)?.headers().iter().any(|h| {
        h.name() == HeaderName::Bcc
            || h.raw_name()
                .trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control())
                .eq_ignore_ascii_case("bcc")
    }) {
        return Err(MailError::Invalid(
            "the Bcc header could not be removed safely, so the draft was not sent".into(),
        ));
    }
    Ok(out)
}

/// SMTP envelope recipients: every To, Cc and Bcc header (a message may repeat them),
/// duplicates removed case-insensitively, order kept.
pub fn envelope_recipients(raw: &[u8]) -> Result<Vec<String>> {
    let m = parse(raw)?;
    let mut out: Vec<String> = Vec::new();
    for header in m.headers() {
        if !matches!(
            header.name(),
            HeaderName::To | HeaderName::Cc | HeaderName::Bcc
        ) {
            continue;
        }
        let HeaderValue::Address(list) = header.value() else {
            continue;
        };
        for addr in list.mailboxes() {
            let Some(email) = addr.address() else {
                continue;
            };
            if !out.iter().any(|e| e.eq_ignore_ascii_case(email)) {
                out.push(email.to_string());
            }
        }
    }
    if out.is_empty() {
        return Err(MailError::Invalid(
            "the draft has no recipients (To, Cc or Bcc)".into(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAFT: &[u8] = b"From: me@icloud.com\r\nTo: Bob <bob@example.com>\r\nBcc: secret@example.com,\r\n  other.secret@example.com\r\nX-Bcc-Note: keep me\r\nbcc: lower@example.com\r\nCc: carol@example.com, BOB@example.com\r\nSubject: Hi\r\n\r\nBody line\r\nBcc: this is body text and must stay\r\n";

    #[test]
    fn strips_all_bcc_headers_including_folded_lines() {
        let out = String::from_utf8(strip_header(DRAFT, "Bcc")).unwrap();
        assert!(!out.contains("secret@example.com"), "{out}");
        assert!(!out.contains("lower@example.com"), "{out}");
        assert!(out.contains("X-Bcc-Note: keep me"));
        assert!(out.contains("To: Bob <bob@example.com>\r\n"));
        assert!(out.contains("Cc: carol@example.com"));
        assert!(out.contains("\r\n\r\nBody line\r\nBcc: this is body text and must stay\r\n"));
    }

    #[test]
    fn strips_bcc_with_whitespace_before_the_colon() {
        let raw = b"To: a@example.com\r\nBcc : hidden@example.com\r\nBCC\t: tab@example.com\r\nBcc-Note: keep\r\nSubject: s\r\n\r\nbody\r\n";
        let r = envelope_recipients(raw).unwrap();
        assert!(r.contains(&"hidden@example.com".to_string()), "{r:?}");
        let out = strip_bcc(raw).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("hidden@example.com"), "{text}");
        assert!(!text.contains("tab@example.com"), "{text}");
        assert!(text.contains("Bcc-Note: keep\r\n"), "{text}");
    }

    #[test]
    fn strips_a_long_folded_bcc_list() {
        let mut raw = b"To: a@example.com\r\nBcc: first@example.com".to_vec();
        for i in 0..40 {
            raw.extend_from_slice(format!(",\r\n\tperson{i}@example.com").as_bytes());
        }
        raw.extend_from_slice(b"\r\nSubject: s\r\n\r\nbody\r\n");
        assert_eq!(envelope_recipients(&raw).unwrap().len(), 42);
        let text = String::from_utf8(strip_bcc(&raw).unwrap()).unwrap();
        assert!(!text.contains("@example.com,"), "{text}");
        assert!(!text.contains("person"), "{text}");
        assert_eq!(text, "To: a@example.com\r\nSubject: s\r\n\r\nbody\r\n");
    }

    #[test]
    fn refuses_when_the_parser_still_sees_a_bcc_header() {
        for raw in [
            &b" Bcc: a@example.com\r\nTo: b@example.com\r\n\r\nx"[..],
            b"To: b@example.com\r\nBcc\r: a@example.com\r\n\r\nx",
        ] {
            assert!(
                matches!(strip_bcc(raw), Err(MailError::Invalid(m)) if m.contains("Bcc")),
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn strip_handles_lf_only_messages() {
        let raw = b"To: a@example.com\nBcc: b@example.com\nSubject: s\n\nbody\n";
        assert_eq!(
            strip_header(raw, "Bcc"),
            b"To: a@example.com\nSubject: s\n\nbody\n".to_vec()
        );
    }

    #[test]
    fn strip_is_noop_without_header() {
        let raw = b"To: a@example.com\r\n\r\nbody";
        assert_eq!(strip_header(raw, "Bcc"), raw.to_vec());
    }

    #[test]
    fn recipients_include_bcc_and_dedupe() {
        let r = envelope_recipients(DRAFT).unwrap();
        assert_eq!(
            r,
            vec![
                "bob@example.com",
                "secret@example.com",
                "other.secret@example.com",
                "lower@example.com",
                "carol@example.com"
            ]
        );
    }

    #[test]
    fn no_recipients_is_an_error() {
        assert!(matches!(
            envelope_recipients(b"From: me@icloud.com\r\nSubject: x\r\n\r\nbody"),
            Err(MailError::Invalid(_))
        ));
    }
}
