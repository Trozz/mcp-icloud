//! Files a tool caller attaches to a new or reply draft: validation and base64 decoding.

use crate::error::{MailError, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_ATTACHMENTS: usize = 10;
const MAX_FILENAME_CHARS: usize = 255;
/// RFC 2045 tspecials: not allowed in a MIME type or subtype token.
const TSPECIALS: &str = "()<>@,;:\\\"/[]?=";

/// One file to attach, as the tool receives it.
#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttachmentInput {
    /// File name shown to the recipient: 1 to 255 characters, without /, \ or control characters.
    pub filename: String,
    /// MIME type as type/subtype, for example application/pdf. Parameters are not accepted.
    pub content_type: String,
    /// The file contents in standard base64. Whitespace is ignored.
    pub content_base64: String,
}

/// A validated, decoded attachment. `Debug` shows its size, never its bytes.
#[derive(Clone)]
pub struct DraftAttachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl fmt::Debug for DraftAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftAttachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("size", &self.data.len())
            .finish()
    }
}

/// What a tool result says about one attachment of a saved draft.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttachmentSummary {
    pub filename: String,
    pub content_type: String,
    pub size: usize,
}

impl DraftAttachment {
    pub fn summary(&self) -> AttachmentSummary {
        AttachmentSummary {
            filename: self.filename.clone(),
            content_type: self.content_type.clone(),
            size: self.data.len(),
        }
    }
}

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_graphic() && !TSPECIALS.contains(c))
}

fn check_filename(n: usize, filename: &str) -> Result<()> {
    if filename.trim().is_empty() || filename.chars().count() > MAX_FILENAME_CHARS {
        return Err(MailError::Invalid(format!(
            "attachment {n} filename must be 1 to {MAX_FILENAME_CHARS} characters"
        )));
    }
    if filename
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err(MailError::Invalid(format!(
            "attachment {n} filename must not contain /, \\ or control characters"
        )));
    }
    Ok(())
}

fn checked_type(n: usize, value: &str) -> Result<String> {
    match value.split_once('/') {
        Some((kind, sub)) if is_token(kind) && is_token(sub) => Ok(value.to_ascii_lowercase()),
        _ => Err(MailError::Invalid(format!(
            "attachment {n} content_type must be type/subtype, such as application/pdf, without parameters"
        ))),
    }
}

fn decoded_data(n: usize, base64: &str) -> Result<Vec<u8>> {
    let compact: String = base64
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    STANDARD
        .decode(compact)
        .map_err(|_| MailError::Invalid(format!("attachment {n} is not valid base64")))
}

/// Validates and decodes every attachment (numbered from 1 in error messages). Returns nothing
/// unless all of them are valid and their decoded total is at most `max_total` bytes.
pub fn decode(inputs: &[AttachmentInput], max_total: usize) -> Result<Vec<DraftAttachment>> {
    if inputs.len() > MAX_ATTACHMENTS {
        return Err(MailError::Invalid(format!(
            "at most {MAX_ATTACHMENTS} attachments are allowed, got {}",
            inputs.len()
        )));
    }
    let mut out = Vec::with_capacity(inputs.len());
    for (i, a) in inputs.iter().enumerate() {
        let n = i + 1;
        check_filename(n, &a.filename)?;
        let content_type = checked_type(n, &a.content_type)?;
        let data = decoded_data(n, &a.content_base64)?;
        out.push(DraftAttachment {
            filename: a.filename.clone(),
            content_type,
            data,
        });
    }
    let total: usize = out.iter().map(|a| a.data.len()).sum();
    if total > max_total {
        return Err(MailError::Invalid(format!(
            "attachments total {total} bytes, larger than MAX_ATTACHMENT_BYTES ({max_total})"
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(filename: &str, content_type: &str, content_base64: &str) -> AttachmentInput {
        AttachmentInput {
            filename: filename.into(),
            content_type: content_type.into(),
            content_base64: content_base64.into(),
        }
    }

    fn err(items: &[AttachmentInput], max_total: usize) -> String {
        match decode(items, max_total) {
            Err(MailError::Invalid(m)) => m,
            Err(other) => panic!("expected an invalid-input error, got {other}"),
            Ok(_) => panic!("expected an error"),
        }
    }

    #[test]
    fn decodes_and_lower_cases_the_type() {
        let out = decode(&[input("notes.txt", "Text/Plain", "aGVsbG8=")], 100).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].filename, "notes.txt");
        assert_eq!(out[0].content_type, "text/plain");
        assert_eq!(out[0].data, b"hello");
        assert_eq!(
            out[0].summary(),
            AttachmentSummary {
                filename: "notes.txt".into(),
                content_type: "text/plain".into(),
                size: 5,
            }
        );
    }

    #[test]
    fn whitespace_in_base64_is_ignored() {
        let out = decode(
            &[input(
                "a.bin",
                "application/octet-stream",
                "aGVs\r\n bG8=\n",
            )],
            100,
        )
        .unwrap();
        assert_eq!(out[0].data, b"hello");
    }

    #[test]
    fn an_empty_list_decodes_to_nothing() {
        assert!(decode(&[], 0).unwrap().is_empty());
    }

    #[test]
    fn at_most_ten_attachments() {
        let list = |n: usize| -> Vec<AttachmentInput> {
            (0..n)
                .map(|i| input(&format!("f{i}.txt"), "text/plain", "eA=="))
                .collect()
        };
        assert_eq!(decode(&list(10), 100).unwrap().len(), 10);
        assert_eq!(
            err(&list(11), 100),
            "at most 10 attachments are allowed, got 11"
        );
    }

    #[test]
    fn total_size_may_reach_the_limit_but_not_pass_it() {
        let two = [
            input("a.txt", "text/plain", "aGVsbG8="),
            input("b.txt", "text/plain", "aGVsbG8="),
        ];
        assert_eq!(decode(&two, 10).unwrap().len(), 2);
        assert_eq!(
            err(&two, 9),
            "attachments total 10 bytes, larger than MAX_ATTACHMENT_BYTES (9)"
        );
    }

    #[test]
    fn filename_rules() {
        let multibyte = "é".repeat(255);
        assert!(decode(&[input(&multibyte, "text/plain", "eA==")], 10).is_ok());
        assert!(decode(&[input("Café menu.txt", "text/plain", "eA==")], 10).is_ok());
        let too_long = "a".repeat(256);
        let length = "attachment 2 filename must be 1 to 255 characters";
        let chars = "attachment 2 filename must not contain /, \\ or control characters";
        for (bad, expected) in [
            ("", length),
            ("   ", length),
            (too_long.as_str(), length),
            ("../a.txt", chars),
            ("a\\b.txt", chars),
            ("a\r\nb.txt", chars),
            ("tab\there.txt", chars),
            ("nul\0.txt", chars),
        ] {
            let items = [
                input("ok.txt", "text/plain", "eA=="),
                input(bad, "text/plain", "eA=="),
            ];
            assert_eq!(err(&items, 10), expected, "{bad:?}");
        }
    }

    #[test]
    fn content_type_rules() {
        for good in [
            "application/pdf",
            "image/svg+xml",
            "application/vnd.ms-excel",
            "text/x-c",
        ] {
            assert!(decode(&[input("f", good, "eA==")], 10).is_ok(), "{good}");
        }
        for bad in [
            "",
            "pdf",
            "application/",
            "/pdf",
            "a/b/c",
            "text/plain; charset=utf-8",
            "text/plain;charset=utf-8",
            "text /plain",
            "text/pl@in",
            "tëxt/plain",
        ] {
            assert_eq!(
                err(&[input("f", bad, "eA==")], 10),
                "attachment 1 content_type must be type/subtype, such as application/pdf, without parameters",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn invalid_base64_names_the_attachment() {
        for bad in ["not base64!", "data:text/plain;base64,aGVsbG8=", "-_-_"] {
            let items = [
                input("ok.txt", "text/plain", "eA=="),
                input("b.txt", "text/plain", bad),
            ];
            assert_eq!(
                err(&items, 100),
                "attachment 2 is not valid base64",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_later_invalid_attachment_fails_the_whole_list() {
        let items = [
            input("ok.txt", "text/plain", "eA=="),
            input("bad/name.txt", "text/plain", "eA=="),
        ];
        assert!(decode(&items, 100).is_err());
    }

    #[test]
    fn debug_never_shows_the_bytes() {
        let a = decode(&[input("s.txt", "text/plain", "c2VjcmV0")], 100).unwrap();
        let shown = format!("{:?}", a[0]);
        assert!(
            !shown.contains("secret") && !shown.contains("115"),
            "{shown}"
        );
        assert!(
            shown.contains("s.txt") && shown.contains("size: 6"),
            "{shown}"
        );
    }
}
