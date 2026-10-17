use super::proto::quote;
use crate::error::{MailError, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub text: Option<String>,
    /// ISO date `YYYY-MM-DD`, inclusive.
    pub since: Option<String>,
    /// ISO date `YYYY-MM-DD`, exclusive.
    pub before: Option<String>,
    pub unseen: Option<bool>,
    pub flagged: Option<bool>,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `2026-10-05` -> `5-Oct-2026` (IMAP date).
pub fn imap_date(iso: &str) -> Result<String> {
    let invalid = || MailError::Invalid(format!("dates must be YYYY-MM-DD, got {iso:?}"));
    let mut parts = iso.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    let y: u32 = y.parse().map_err(|_| invalid())?;
    let m: usize = m.parse().map_err(|_| invalid())?;
    let d: u32 = d.parse().map_err(|_| invalid())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1970..=9999).contains(&y) {
        return Err(invalid());
    }
    Ok(format!("{d}-{}-{y}", MONTHS[m - 1]))
}

/// Builds `UID SEARCH` criteria. `below_uid` restricts to UIDs strictly below it (pagination).
/// Returns `None` when the page is provably empty (cursor at uid 1).
pub fn build_search(q: &SearchQuery, below_uid: Option<u32>) -> Result<Option<String>> {
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in [
        ("FROM", &q.from),
        ("TO", &q.to),
        ("SUBJECT", &q.subject),
        ("TEXT", &q.text),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            parts.push(format!("{key} {}", quote(v)?));
        }
    }
    if let Some(d) = &q.since {
        parts.push(format!("SINCE {}", imap_date(d)?));
    }
    if let Some(d) = &q.before {
        parts.push(format!("BEFORE {}", imap_date(d)?));
    }
    match q.unseen {
        Some(true) => parts.push("UNSEEN".into()),
        Some(false) => parts.push("SEEN".into()),
        None => {}
    }
    match q.flagged {
        Some(true) => parts.push("FLAGGED".into()),
        Some(false) => parts.push("UNFLAGGED".into()),
        None => {}
    }
    match below_uid {
        Some(0 | 1) => return Ok(None),
        Some(n) => parts.push(format!("UID 1:{}", n - 1)),
        None => {}
    }
    parts.push("UNDELETED".into());
    let criteria = parts.join(" ");
    if criteria.is_ascii() {
        return Ok(Some(criteria));
    }
    Ok(Some(format!("CHARSET UTF-8 {criteria}")))
}

pub fn encode_cursor(uidvalidity: u32, last_uid: u32) -> String {
    URL_SAFE_NO_PAD.encode(format!("{uidvalidity}:{last_uid}"))
}

pub fn decode_cursor(cursor: &str) -> Result<(u32, u32)> {
    let invalid = || MailError::Invalid("invalid cursor".into());
    let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let s = String::from_utf8(bytes).map_err(|_| invalid())?;
    let (uv, uid) = s.split_once(':').ok_or_else(invalid)?;
    Ok((
        uv.parse().map_err(|_| invalid())?,
        uid.parse().map_err(|_| invalid())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_undeleted() {
        assert_eq!(
            build_search(&SearchQuery::default(), None)
                .unwrap()
                .unwrap(),
            "UNDELETED"
        );
    }

    #[test]
    fn all_criteria() {
        let q = SearchQuery {
            from: Some("alice".into()),
            to: Some("bob@example.com".into()),
            subject: Some("say \"hi\"".into()),
            text: Some("invoice".into()),
            since: Some("2026-10-05".into()),
            before: Some("2026-11-01".into()),
            unseen: Some(true),
            flagged: Some(false),
        };
        assert_eq!(
            build_search(&q, Some(50)).unwrap().unwrap(),
            "FROM \"alice\" TO \"bob@example.com\" SUBJECT \"say \\\"hi\\\"\" TEXT \"invoice\" SINCE 5-Oct-2026 BEFORE 1-Nov-2026 UNSEEN UNFLAGGED UID 1:49 UNDELETED"
        );
    }

    #[test]
    fn non_ascii_uses_utf8_charset() {
        let q = SearchQuery {
            subject: Some("caf\u{e9}".into()),
            ..Default::default()
        };
        assert_eq!(
            build_search(&q, None).unwrap().unwrap(),
            "CHARSET UTF-8 SUBJECT \"caf\u{e9}\" UNDELETED"
        );
    }

    #[test]
    fn line_breaks_and_bad_dates_are_rejected() {
        assert!(
            build_search(
                &SearchQuery {
                    from: Some("a\r\nUID 1:*".into()),
                    ..Default::default()
                },
                None
            )
            .is_err()
        );
        assert!(
            build_search(
                &SearchQuery {
                    since: Some("05/10/2026".into()),
                    ..Default::default()
                },
                None
            )
            .is_err()
        );
        assert!(
            build_search(
                &SearchQuery {
                    since: Some("2026-13-01".into()),
                    ..Default::default()
                },
                None
            )
            .is_err()
        );
    }

    #[test]
    fn cursor_at_first_uid_is_an_empty_page() {
        assert_eq!(
            build_search(&SearchQuery::default(), Some(1)).unwrap(),
            None
        );
    }

    #[test]
    fn cursor_roundtrip_and_garbage() {
        assert_eq!(decode_cursor(&encode_cursor(1000, 42)).unwrap(), (1000, 42));
        assert!(decode_cursor("!!!").is_err());
        assert!(decode_cursor(&URL_SAFE_NO_PAD.encode("x:y")).is_err());
    }
}
