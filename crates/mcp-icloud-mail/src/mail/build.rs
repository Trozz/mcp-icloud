use super::attach::{AttachmentSummary, DraftAttachment};
use super::parse::{display_text, format_addresses, header_ids, parse};
use crate::error::{MailError, Result};
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Identity {
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DraftInput {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub attachments: Vec<DraftAttachment>,
}

pub struct BuiltMessage {
    pub raw: Vec<u8>,
    pub message_id: String,
    /// The files attached to the message, in order.
    pub attachments: Vec<AttachmentSummary>,
}

/// Parses "Name <a@b>" or "a@b" into (name, email), validating the address.
pub fn parse_mailbox(s: &str) -> Result<(Option<String>, String)> {
    let mb = lettre::message::Mailbox::from_str(s.trim())
        .map_err(|e| MailError::Invalid(format!("invalid email address {s:?}: {e}")))?;
    Ok((mb.name.filter(|n| !n.is_empty()), mb.email.to_string()))
}

fn address_list(list: &[String]) -> Result<Vec<Address<'static>>> {
    list.iter()
        .map(|s| {
            let (name, email) = parse_mailbox(s)?;
            Ok(Address::new_address(name, email))
        })
        .collect()
}

fn new_message_id(identity: &Identity) -> String {
    let domain = identity
        .email
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or("localhost");
    format!("{}@{}", uuid::Uuid::new_v4(), domain)
}

fn base(identity: &Identity, message_id: &str) -> MessageBuilder<'static> {
    let from = Address::new_address(identity.name.clone(), identity.email.clone());
    MessageBuilder::new()
        .from(from)
        .message_id(message_id.to_string())
}

fn finish(
    builder: MessageBuilder<'_>,
    message_id: String,
    attachments: Vec<AttachmentSummary>,
) -> Result<BuiltMessage> {
    let raw = builder
        .write_to_vec()
        .map_err(|e| MailError::Invalid(format!("could not build message: {e}")))?;
    Ok(BuiltMessage {
        raw,
        message_id,
        attachments,
    })
}

/// Adds caller-supplied attachments and returns what was attached.
fn with_attachments(
    mut b: MessageBuilder<'static>,
    attachments: &[DraftAttachment],
) -> (MessageBuilder<'static>, Vec<AttachmentSummary>) {
    for a in attachments {
        b = b.attachment(a.content_type.clone(), a.filename.clone(), a.data.clone());
    }
    (
        b,
        attachments.iter().map(DraftAttachment::summary).collect(),
    )
}

fn with_recipients(
    mut b: MessageBuilder<'static>,
    to: &[String],
    cc: &[String],
    bcc: &[String],
) -> Result<MessageBuilder<'static>> {
    if !to.is_empty() {
        b = b.to(address_list(to)?);
    }
    if !cc.is_empty() {
        b = b.cc(address_list(cc)?);
    }
    if !bcc.is_empty() {
        b = b.bcc(address_list(bcc)?);
    }
    Ok(b)
}

pub fn build_draft(identity: &Identity, d: &DraftInput) -> Result<BuiltMessage> {
    let id = new_message_id(identity);
    let mut b = with_recipients(base(identity, &id), &d.to, &d.cc, &d.bcc)?
        .subject(d.subject.clone())
        .text_body(d.body_text.clone());
    if let Some(html) = &d.body_html {
        b = b.html_body(html.clone());
    }
    let (b, attached) = with_attachments(b, &d.attachments);
    finish(b, id, attached)
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let lower = subject.trim_start().to_ascii_lowercase();
    if lower.starts_with(&format!("{}:", prefix.to_ascii_lowercase())) {
        return subject.to_string();
    }
    format!("{prefix}: {subject}")
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn email_of(s: &str) -> String {
    parse_mailbox(s)
        .map(|(_, e)| e)
        .unwrap_or_else(|_| s.to_string())
        .to_ascii_lowercase()
}

/// Builds a reply draft. Recipients: Reply-To (else From). reply_all adds the original To and Cc,
/// minus our own addresses and duplicates. Replying to our own message goes to its original To.
/// Caller-supplied attachments are added after the quoted text.
pub fn build_reply(
    identity: &Identity,
    self_addresses: &[String],
    original: &[u8],
    body: &str,
    reply_all: bool,
    attachments: &[DraftAttachment],
) -> Result<BuiltMessage> {
    let m = parse(original)?;
    let me: Vec<String> = self_addresses
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let is_me = |s: &String| me.contains(&email_of(s));
    let mut to: Vec<String> = format_addresses(m.reply_to());
    if to.is_empty() {
        to = format_addresses(m.from());
    }
    if to.iter().all(is_me) {
        to = format_addresses(m.to());
    }
    to.retain(|s| !is_me(s));
    let mut cc = Vec::new();
    if reply_all {
        for s in format_addresses(m.to())
            .into_iter()
            .chain(format_addresses(m.cc()))
        {
            let e = email_of(&s);
            if !is_me(&s) && !to.iter().chain(cc.iter()).any(|x| email_of(x) == e) {
                cc.push(s);
            }
        }
    }
    let mut references = header_ids(m.references());
    let original_id = m.message_id().map(str::to_string);
    if let Some(id) = &original_id
        && !references.contains(id)
    {
        references.push(id.clone());
    }
    let quoted_from = format_addresses(m.from()).join(", ");
    let date = m.date().map(|d| d.to_rfc822()).unwrap_or_default();
    let text = format!(
        "{body}\n\nOn {date}, {quoted_from} wrote:\n{}\n",
        quote(&display_text(&m))
    );

    let id = new_message_id(identity);
    let mut b = with_recipients(base(identity, &id), &to, &cc, &[])?
        .subject(prefixed("Re", m.subject().unwrap_or_default()))
        .text_body(text);
    if let Some(orig) = original_id {
        b = b.in_reply_to(orig);
    }
    if !references.is_empty() {
        b = b.references(references);
    }
    let (b, attached) = with_attachments(b, attachments);
    finish(b, id, attached)
}

/// Builds a forward draft with a forwarded-message header block and, optionally, the original attachments.
pub fn build_forward(
    identity: &Identity,
    original: &[u8],
    to: &[String],
    body: &str,
    include_attachments: bool,
) -> Result<BuiltMessage> {
    let m = parse(original)?;
    let header = format!(
        "---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n",
        format_addresses(m.from()).join(", "),
        m.date().map(|d| d.to_rfc822()).unwrap_or_default(),
        m.subject().unwrap_or_default(),
        format_addresses(m.to()).join(", "),
    );
    let text = format!("{body}\n\n{header}\n{}\n", display_text(&m));
    let id = new_message_id(identity);
    let mut b = with_recipients(base(identity, &id), to, &[], &[])?
        .subject(prefixed("Fwd", m.subject().unwrap_or_default()))
        .text_body(text);
    let mut attached = Vec::new();
    if include_attachments {
        for a in super::parse::attachments(&m) {
            let part = m
                .attachments()
                .nth(a.index)
                .expect("index from attachments()");
            let filename = a
                .filename
                .clone()
                .unwrap_or_else(|| format!("attachment-{}", a.index));
            b = b.attachment(
                a.mime_type.clone(),
                filename.clone(),
                part.decoded().into_owned(),
            );
            attached.push(AttachmentSummary {
                filename,
                content_type: a.mime_type,
                size: a.size,
            });
        }
    }
    finish(b, id, attached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::attach::{AttachmentSummary, DraftAttachment};
    use crate::mail::parse::{BodyFormat, attachment, view};

    fn me() -> Identity {
        Identity {
            email: "me@icloud.com".into(),
            name: Some("Me Myself".into()),
        }
    }

    fn file(filename: &str, content_type: &str, data: &[u8]) -> DraftAttachment {
        DraftAttachment {
            filename: filename.into(),
            content_type: content_type.into(),
            data: data.to_vec(),
        }
    }

    const ORIGINAL: &[u8] = b"From: Alice <alice@example.com>\r\nTo: me@icloud.com, Bob <bob@example.com>\r\nCc: carol@example.com, me@icloud.com\r\nSubject: Plans\r\nDate: Wed, 15 Oct 2026 10:00:00 +0000\r\nMessage-ID: <orig@example.com>\r\nReferences: <root@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nLine one\r\nLine two\r\n";

    const WITH_ATTACHMENT: &[u8] = b"From: a@example.com\r\nTo: me@icloud.com\r\nSubject: report\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--b1\r\nContent-Type: text/csv; name=\"data.csv\"\r\nContent-Disposition: attachment; filename=\"data.csv\"\r\n\r\na,b\r\n--b1--\r\n";

    #[test]
    fn parse_mailbox_forms() {
        assert_eq!(
            parse_mailbox("bob@example.com").unwrap(),
            (None, "bob@example.com".into())
        );
        assert_eq!(
            parse_mailbox("Bob B <bob@example.com>").unwrap(),
            (Some("Bob B".into()), "bob@example.com".into())
        );
        assert!(parse_mailbox("not an address").is_err());
    }

    #[test]
    fn draft_contains_all_headers_and_bodies() {
        let d = DraftInput {
            to: vec!["Bob <bob@example.com>".into()],
            cc: vec!["carol@example.com".into()],
            bcc: vec!["secret@example.com".into()],
            subject: "Hello".into(),
            body_text: "Plain body".into(),
            body_html: Some("<p>Html body</p>".into()),
            attachments: Vec::new(),
        };
        let built = build_draft(&me(), &d).unwrap();
        assert!(built.message_id.ends_with("@icloud.com"));
        let v = view(&built.raw, BodyFormat::Text, 10_000).unwrap();
        assert_eq!(v.from, vec!["Me Myself <me@icloud.com>"]);
        assert_eq!(v.to, vec!["Bob <bob@example.com>"]);
        assert_eq!(v.cc, vec!["carol@example.com"]);
        assert_eq!(v.subject, "Hello");
        assert_eq!(v.message_id.as_deref(), Some(built.message_id.as_str()));
        assert!(v.body.contains("Plain body"));
        let raw = String::from_utf8_lossy(&built.raw);
        assert!(
            raw.contains("Bcc: <secret@example.com>") || raw.contains("Bcc: secret@example.com"),
            "{raw}"
        );
        assert!(raw.contains("<p>Html body</p>"));
    }

    #[test]
    fn draft_rejects_invalid_recipient() {
        let d = DraftInput {
            to: vec!["nope".into()],
            ..Default::default()
        };
        assert!(matches!(build_draft(&me(), &d), Err(MailError::Invalid(_))));
    }

    #[test]
    fn draft_without_recipients_is_allowed() {
        let built = build_draft(
            &me(),
            &DraftInput {
                subject: "Note".into(),
                body_text: "x".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            view(&built.raw, BodyFormat::Text, 100)
                .unwrap()
                .to
                .is_empty()
        );
    }

    #[test]
    fn reply_threads_and_quotes() {
        let built = build_reply(
            &me(),
            &["me@icloud.com".into()],
            ORIGINAL,
            "Sounds good",
            false,
            &[],
        )
        .unwrap();
        let v = view(&built.raw, BodyFormat::Text, 10_000).unwrap();
        assert_eq!(v.subject, "Re: Plans");
        assert_eq!(v.to, vec!["Alice <alice@example.com>"]);
        assert!(v.cc.is_empty());
        assert_eq!(v.in_reply_to, vec!["orig@example.com"]);
        assert_eq!(v.references, vec!["root@example.com", "orig@example.com"]);
        assert!(v.body.starts_with("Sounds good"));
        assert!(v.body.contains("> Line one"));
        assert!(v.body.contains("wrote:"));
    }

    #[test]
    fn reply_all_excludes_self_and_duplicates() {
        let built =
            build_reply(&me(), &["ME@icloud.com".into()], ORIGINAL, "ok", true, &[]).unwrap();
        let v = view(&built.raw, BodyFormat::Text, 10_000).unwrap();
        assert_eq!(v.to, vec!["Alice <alice@example.com>"]);
        assert_eq!(v.cc, vec!["Bob <bob@example.com>", "carol@example.com"]);
    }

    #[test]
    fn reply_prefers_reply_to() {
        let orig = b"From: list@example.com\r\nReply-To: Real <real@example.com>\r\nTo: me@icloud.com\r\nSubject: re: Topic\r\nMessage-ID: <x@example.com>\r\n\r\nbody\r\n";
        let v = view(
            &build_reply(&me(), &["me@icloud.com".into()], orig, "hi", false, &[])
                .unwrap()
                .raw,
            BodyFormat::Text,
            1000,
        )
        .unwrap();
        assert_eq!(v.to, vec!["Real <real@example.com>"]);
        assert_eq!(v.subject, "re: Topic");
    }

    #[test]
    fn reply_to_own_message_goes_to_original_recipients() {
        let orig = b"From: me@icloud.com\r\nTo: Bob <bob@example.com>\r\nSubject: Mine\r\nMessage-ID: <mine@icloud.com>\r\n\r\nbody\r\n";
        let v = view(
            &build_reply(
                &me(),
                &["me@icloud.com".into()],
                orig,
                "follow up",
                false,
                &[],
            )
            .unwrap()
            .raw,
            BodyFormat::Text,
            1000,
        )
        .unwrap();
        assert_eq!(v.to, vec!["Bob <bob@example.com>"]);
    }

    #[test]
    fn forward_includes_header_block_and_attachments() {
        let built = build_forward(
            &me(),
            WITH_ATTACHMENT,
            &["dave@example.com".into()],
            "FYI",
            true,
        )
        .unwrap();
        let v = view(&built.raw, BodyFormat::Text, 10_000).unwrap();
        assert_eq!(v.subject, "Fwd: report");
        assert_eq!(v.to, vec!["dave@example.com"]);
        assert!(v.body.starts_with("FYI"));
        assert!(v.body.contains("---------- Forwarded message ----------"));
        assert!(v.body.contains("See attached."));
        assert_eq!(v.attachments.len(), 1);
        assert_eq!(v.attachments[0].filename.as_deref(), Some("data.csv"));
        let reported: Vec<(&str, &str)> = built
            .attachments
            .iter()
            .map(|a| (a.filename.as_str(), a.content_type.as_str()))
            .collect();
        assert_eq!(reported, [("data.csv", "text/csv")]);
    }

    #[test]
    fn forward_without_attachments() {
        let built = build_forward(
            &me(),
            WITH_ATTACHMENT,
            &["dave@example.com".into()],
            "FYI",
            false,
        )
        .unwrap();
        assert!(built.attachments.is_empty());
        assert!(
            view(&built.raw, BodyFormat::Text, 10_000)
                .unwrap()
                .attachments
                .is_empty()
        );
    }

    #[test]
    fn draft_carries_attachments_with_names_types_and_bytes() {
        let d = DraftInput {
            to: vec!["bob@example.com".into()],
            subject: "Files".into(),
            body_text: "Two files attached.".into(),
            body_html: Some("<p>Two files attached.</p>".into()),
            attachments: vec![
                file("report.pdf", "application/pdf", b"%PDF-1.4\n"),
                file("Café menu.txt", "text/plain", b"soup\n"),
            ],
            ..Default::default()
        };
        let built = build_draft(&me(), &d).unwrap();
        let v = view(&built.raw, BodyFormat::Text, 10_000).unwrap();
        assert!(v.body.contains("Two files attached."));
        let parts: Vec<(Option<&str>, &str)> = v
            .attachments
            .iter()
            .map(|a| (a.filename.as_deref(), a.mime_type.as_str()))
            .collect();
        assert_eq!(
            parts,
            [
                (Some("report.pdf"), "application/pdf"),
                (Some("Café menu.txt"), "text/plain")
            ]
        );
        assert_eq!(attachment(&built.raw, 0).unwrap().contents, b"%PDF-1.4\n");
        assert_eq!(attachment(&built.raw, 1).unwrap().contents, b"soup\n");
        assert_eq!(
            built.attachments,
            vec![
                AttachmentSummary {
                    filename: "report.pdf".into(),
                    content_type: "application/pdf".into(),
                    size: 9,
                },
                AttachmentSummary {
                    filename: "Café menu.txt".into(),
                    content_type: "text/plain".into(),
                    size: 5,
                },
            ]
        );
    }

    #[test]
    fn draft_without_attachments_is_not_multipart_mixed() {
        let built = build_draft(
            &me(),
            &DraftInput {
                subject: "Note".into(),
                body_text: "x".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(built.attachments.is_empty());
        assert!(!String::from_utf8_lossy(&built.raw).contains("multipart/mixed"));
    }

    #[test]
    fn reply_with_attachment_keeps_threading_and_quote() {
        let built = build_reply(
            &me(),
            &["me@icloud.com".into()],
            ORIGINAL,
            "Signed copy attached",
            false,
            &[file("signed.pdf", "application/pdf", b"%PDF-1.4\n")],
        )
        .unwrap();
        let v = view(&built.raw, BodyFormat::Text, 10_000).unwrap();
        assert_eq!(v.subject, "Re: Plans");
        assert_eq!(v.to, vec!["Alice <alice@example.com>"]);
        assert_eq!(v.in_reply_to, vec!["orig@example.com"]);
        assert_eq!(v.references, vec!["root@example.com", "orig@example.com"]);
        assert!(v.body.starts_with("Signed copy attached"));
        assert!(v.body.contains("> Line one"));
        assert_eq!(v.attachments.len(), 1);
        assert_eq!(v.attachments[0].filename.as_deref(), Some("signed.pdf"));
        assert_eq!(attachment(&built.raw, 0).unwrap().contents, b"%PDF-1.4\n");
        assert_eq!(built.attachments.len(), 1);
    }
}
