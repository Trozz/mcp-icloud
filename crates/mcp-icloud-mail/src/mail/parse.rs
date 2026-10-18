use crate::error::{MailError, Result};
use mail_parser::{HeaderValue, Message, MessageParser, MimeHeaders};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFormat {
    Text,
    Html,
    Raw,
}

impl BodyFormat {
    pub fn parse(s: Option<&str>) -> Result<Self> {
        match s.unwrap_or("text") {
            "text" => Ok(BodyFormat::Text),
            "html" => Ok(BodyFormat::Html),
            "raw" => Ok(BodyFormat::Raw),
            other => Err(MailError::Invalid(format!(
                "format must be text, html or raw, got {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttachmentInfo {
    pub index: usize,
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub subject: String,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub reply_to: Vec<String>,
    pub date: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub body: String,
    pub body_format: &'static str,
    pub truncated: bool,
    pub attachments: Vec<AttachmentInfo>,
}

pub struct Attachment {
    pub filename: Option<String>,
    pub mime_type: String,
    pub contents: Vec<u8>,
}

/// Decodes RFC 2047 encoded words (as returned undecoded in IMAP ENVELOPE fields).
pub fn decode_header_words(raw: &[u8]) -> String {
    let mut buf = raw.to_vec();
    buf.extend_from_slice(b"\r\n");
    match mail_parser::parsers::MessageStream::new(&buf).parse_unstructured() {
        HeaderValue::Text(t) => t.into_owned(),
        _ => String::from_utf8_lossy(raw).into_owned(),
    }
}

pub fn html_to_text(html: &str) -> String {
    html2text::config::plain_no_decorate()
        .string_from_read(html.as_bytes(), 100)
        .unwrap_or_else(|_| mail_parser::decoders::html::html_to_text(html))
}

pub(crate) fn parse(raw: &[u8]) -> Result<Message<'_>> {
    MessageParser::default()
        .parse(raw)
        .ok_or_else(|| MailError::Invalid("message could not be parsed".into()))
}

pub(crate) fn format_addresses(a: Option<&mail_parser::Address<'_>>) -> Vec<String> {
    let Some(a) = a else { return Vec::new() };
    a.iter()
        .filter_map(|addr| {
            let email = addr.address()?;
            Some(match addr.name() {
                Some(name) if !name.is_empty() => format!("{name} <{email}>"),
                _ => email.to_string(),
            })
        })
        .collect()
}

pub(crate) fn header_ids(v: &HeaderValue<'_>) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn has_text_part(m: &Message<'_>, subtype: &str) -> bool {
    m.parts.iter().any(|p| {
        let matches = match p.content_type() {
            Some(ct) => {
                ct.ctype().eq_ignore_ascii_case("text")
                    && ct
                        .subtype()
                        .is_some_and(|s| s.eq_ignore_ascii_case(subtype))
            }
            None => subtype == "plain",
        };
        matches && p.is_text() && p.attachment_name().is_none()
    })
}

/// Plain text for display: the text/plain part if any, otherwise the HTML part converted to text.
/// mail-parser's body_text/body_html synthesise one from the other, so check real parts first.
pub(crate) fn display_text(m: &Message<'_>) -> String {
    if has_text_part(m, "plain") {
        return m.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    }
    if has_text_part(m, "html") {
        return m.body_html(0).map(|h| html_to_text(&h)).unwrap_or_default();
    }
    String::new()
}

fn truncate(s: String, max_chars: usize) -> (String, bool) {
    if s.chars().count() <= max_chars {
        return (s, false);
    }
    (s.chars().take(max_chars).collect(), true)
}

pub fn attachments(m: &Message<'_>) -> Vec<AttachmentInfo> {
    m.attachments()
        .enumerate()
        .map(|(index, p)| AttachmentInfo {
            index,
            filename: p.attachment_name().map(str::to_string),
            mime_type: p
                .content_type()
                .map(|c| match c.subtype() {
                    Some(sub) => format!("{}/{}", c.ctype(), sub),
                    None => c.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into())
                .to_ascii_lowercase(),
            size: p.contents().len(),
        })
        .collect()
}

pub fn view(raw: &[u8], format: BodyFormat, max_chars: usize) -> Result<MessageView> {
    let m = parse(raw)?;
    let (body, body_format) = match format {
        BodyFormat::Raw => (String::from_utf8_lossy(raw).into_owned(), "raw"),
        BodyFormat::Html if has_text_part(&m, "html") => (
            m.body_html(0).map(|h| h.into_owned()).unwrap_or_default(),
            "html",
        ),
        BodyFormat::Html | BodyFormat::Text => (display_text(&m), "text"),
    };
    let (body, truncated) = truncate(body, max_chars);
    Ok(MessageView {
        subject: m.subject().unwrap_or_default().to_string(),
        from: format_addresses(m.from()),
        to: format_addresses(m.to()),
        cc: format_addresses(m.cc()),
        reply_to: format_addresses(m.reply_to()),
        date: m.date().map(|d| d.to_rfc3339()),
        message_id: m.message_id().map(str::to_string),
        in_reply_to: header_ids(m.in_reply_to()),
        references: header_ids(m.references()),
        body,
        body_format,
        truncated,
        attachments: attachments(&m),
    })
}

pub fn attachment(raw: &[u8], index: usize) -> Result<Attachment> {
    let m = parse(raw)?;
    let info = attachments(&m)
        .into_iter()
        .nth(index)
        .ok_or_else(|| MailError::Invalid(format!("attachment index {index} does not exist")))?;
    let part = m.attachments().nth(index).expect("index checked above");
    Ok(Attachment {
        filename: info.filename,
        mime_type: info.mime_type,
        contents: part.contents().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: Alice Example <alice@example.com>\r\nTo: me@icloud.com, Bob <bob@example.com>\r\nCc: carol@example.com\r\nSubject: =?UTF-8?Q?Caf=C3=A9_plans?=\r\nDate: Wed, 15 Oct 2026 10:00:00 +0000\r\nMessage-ID: <m1@example.com>\r\nReferences: <r0@example.com> <r1@example.com>\r\nIn-Reply-To: <r1@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello there,\r\nsee you at the caf\xc3\xa9.\r\n";

    const HTML_ONLY: &[u8] = b"From: a@example.com\r\nTo: me@icloud.com\r\nSubject: html\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Hello <b>world</b></p><p>Second</p></body></html>\r\n";

    const ATTACHMENT_ONLY: &[u8] = b"From: a@example.com\r\nTo: me@icloud.com\r\nSubject: scan\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: application/pdf; name=\"scan.pdf\"\r\nContent-Disposition: attachment; filename=\"scan.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b1--\r\n";

    const MIXED: &[u8] = b"From: a@example.com\r\nTo: me@icloud.com\r\nSubject: report\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSee attached.\r\n--b1\r\nContent-Type: text/csv; name=\"data.csv\"\r\nContent-Disposition: attachment; filename=\"data.csv\"\r\n\r\na,b\r\n1,2\r\n--b1--\r\n";

    #[test]
    fn decodes_q_and_b_encoded_words() {
        assert_eq!(
            decode_header_words(b"=?UTF-8?Q?Caf=C3=A9_plans?="),
            "Caf\u{e9} plans"
        );
        assert_eq!(decode_header_words(b"=?UTF-8?B?Q2Fmw6k=?="), "Caf\u{e9}");
        assert_eq!(decode_header_words(b"plain subject"), "plain subject");
    }

    #[test]
    fn plain_message_view() {
        let v = view(PLAIN, BodyFormat::Text, 20_000).unwrap();
        assert_eq!(v.subject, "Caf\u{e9} plans");
        assert_eq!(v.from, vec!["Alice Example <alice@example.com>"]);
        assert_eq!(v.to, vec!["me@icloud.com", "Bob <bob@example.com>"]);
        assert_eq!(v.cc, vec!["carol@example.com"]);
        assert_eq!(v.message_id.as_deref(), Some("m1@example.com"));
        assert_eq!(v.references, vec!["r0@example.com", "r1@example.com"]);
        assert_eq!(v.in_reply_to, vec!["r1@example.com"]);
        assert!(v.body.contains("see you at the caf\u{e9}"));
        assert_eq!(v.body_format, "text");
        assert!(!v.truncated);
        assert!(v.attachments.is_empty());
        assert!(v.date.unwrap().starts_with("2026-10-15T10:00:00"));
    }

    #[test]
    fn html_only_message_is_converted_to_text() {
        let v = view(HTML_ONLY, BodyFormat::Text, 20_000).unwrap();
        assert!(v.body.contains("Hello world"), "{}", v.body);
        assert!(!v.body.contains("<b>"));
        let h = view(HTML_ONLY, BodyFormat::Html, 20_000).unwrap();
        assert!(h.body.contains("<b>world</b>"));
        assert_eq!(h.body_format, "html");
    }

    #[test]
    fn html_format_falls_back_to_text_when_no_html_part() {
        let v = view(PLAIN, BodyFormat::Html, 20_000).unwrap();
        assert_eq!(v.body_format, "text");
        assert!(v.body.contains("Hello there"));
    }

    #[test]
    fn attachment_only_message_has_empty_body_and_lists_attachment() {
        let v = view(ATTACHMENT_ONLY, BodyFormat::Text, 20_000).unwrap();
        assert_eq!(v.body, "");
        assert_eq!(v.attachments.len(), 1);
        assert_eq!(v.attachments[0].filename.as_deref(), Some("scan.pdf"));
        assert_eq!(v.attachments[0].mime_type, "application/pdf");
        let a = attachment(ATTACHMENT_ONLY, 0).unwrap();
        assert!(a.contents.starts_with(b"%PDF-1.4"));
    }

    #[test]
    fn mixed_message_keeps_text_body_and_attachment() {
        let v = view(MIXED, BodyFormat::Text, 20_000).unwrap();
        assert!(v.body.contains("See attached."));
        assert!(!v.body.contains("a,b"));
        assert_eq!(
            v.attachments,
            vec![AttachmentInfo {
                index: 0,
                filename: Some("data.csv".into()),
                mime_type: "text/csv".into(),
                size: 8
            }]
        );
        assert_eq!(
            attachment(MIXED, 0).unwrap().contents,
            b"a,b\r\n1,2".to_vec()
        );
    }

    #[test]
    fn missing_attachment_index_is_invalid() {
        assert!(matches!(attachment(MIXED, 5), Err(MailError::Invalid(_))));
    }

    #[test]
    fn body_is_truncated_by_chars() {
        let v = view(PLAIN, BodyFormat::Text, 5).unwrap();
        assert_eq!(v.body, "Hello");
        assert!(v.truncated);
    }

    #[test]
    fn raw_format_returns_source() {
        let v = view(PLAIN, BodyFormat::Raw, 20_000).unwrap();
        assert!(v.body.starts_with("From: Alice Example"));
    }

    #[test]
    fn format_parsing() {
        assert_eq!(BodyFormat::parse(None).unwrap(), BodyFormat::Text);
        assert_eq!(BodyFormat::parse(Some("html")).unwrap(), BodyFormat::Html);
        assert!(BodyFormat::parse(Some("pdf")).is_err());
    }
}
