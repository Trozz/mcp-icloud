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
            skipping = line.len() > name.len()
                && line[..name.len()].eq_ignore_ascii_case(name.as_bytes())
                && line[name.len()] == b':';
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(rest);
    out
}

/// SMTP envelope recipients: every To, Cc and Bcc header (a message may repeat them),
/// duplicates removed case-insensitively, order kept.
pub fn envelope_recipients(raw: &[u8]) -> Result<Vec<String>> {
    let m = parse(raw)?;
    let mut out: Vec<String> = Vec::new();
    for header in m.headers() {
        if !matches!(
            header.name,
            HeaderName::To | HeaderName::Cc | HeaderName::Bcc
        ) {
            continue;
        }
        let HeaderValue::Address(list) = &header.value else {
            continue;
        };
        for addr in list.iter() {
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
