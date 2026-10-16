//! Low-level helpers over an authenticated async-imap session.
//!
//! async-imap ends streaming commands (UID FETCH, UID STORE, LIST, UID EXPUNGE) silently when the
//! server answers with a tagged NO/BAD, so every mutating command is sent with `run_checked`,
//! which reads responses up to the tagged completion and checks its status.

use crate::error::{MailError, Result};
use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::imap_proto::{MailboxDatum, Outcome, RequestId, Response, ResponseCode, Status};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub trait ImapStream: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug> ImapStream for T {}

pub type ImapSession = async_imap::Session<Box<dyn ImapStream>>;

/// Quotes an IMAP string. CR and LF cannot appear in a quoted string and are rejected.
pub fn quote(s: &str) -> Result<String> {
    if s.contains(['\r', '\n']) {
        return Err(MailError::Invalid(
            "values must not contain line breaks".into(),
        ));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Outcome of a tagged OK: the APPENDUID uid, if the server sent one.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub append_uid: Option<u32>,
}

/// Maps a tagged completion to `Done` (OK) or `MailError::Rejected` (NO/BAD).
fn completion(label: &str, status: &Status, outcome: &Outcome<'_>) -> Result<Done> {
    if *status != Status::Ok {
        return Err(MailError::rejected(
            label,
            outcome
                .information
                .as_deref()
                .unwrap_or("no details")
                .to_string(),
        ));
    }
    Ok(Done {
        append_uid: match &outcome.code {
            Some(ResponseCode::AppendUid(_, set)) => set
                .iter()
                .map(|m| match m {
                    UidSetMember::Uid(u) => *u,
                    UidSetMember::UidRange(r) => *r.start(),
                })
                .next(),
            _ => None,
        },
    })
}

/// Reads responses until the completion tagged `id` and maps NO/BAD to `MailError::Rejected`.
/// Completions carrying any other tag are not ours and are skipped.
pub async fn read_done(s: &mut ImapSession, label: &str, id: &RequestId) -> Result<Done> {
    loop {
        let Some(resp) = s.read_response().await? else {
            return Err(MailError::Connection("connection closed by server".into()));
        };
        if let Response::Done {
            tag,
            status,
            outcome,
        } = resp.parsed()
            && tag == id
        {
            return completion(label, status, outcome);
        }
    }
}

pub async fn run_checked(s: &mut ImapSession, label: &str, command: &str) -> Result<Done> {
    let id = s.run_command(command).await?;
    read_done(s, label, &id).await
}

/// `UID SEARCH`, written by hand because async-imap's `uid_search` returns an empty result on a
/// tagged NO/BAD. Returns the matching uids in server order.
pub async fn uid_search(s: &mut ImapSession, criteria: &str) -> Result<Vec<u32>> {
    let id = s.run_command(format!("UID SEARCH {criteria}")).await?;
    let mut uids = Vec::new();
    loop {
        let Some(resp) = s.read_response().await? else {
            return Err(MailError::Connection("connection closed by server".into()));
        };
        match resp.parsed() {
            Response::MailboxData(MailboxDatum::Search(ids)) => uids.extend_from_slice(ids),
            Response::Done {
                tag,
                status,
                outcome,
            } if tag == &id => {
                completion("UID SEARCH", status, outcome)?;
                return Ok(uids);
            }
            _ => {}
        }
    }
}

/// APPEND with a synchronizing literal, written by hand because async-imap's `append()` drops the
/// APPENDUID response code. Returns the tagged completion, including APPENDUID when sent.
pub async fn append_literal(
    s: &mut ImapSession,
    mailbox: &str,
    flags: &str,
    raw: &[u8],
) -> Result<Done> {
    let command = format!("APPEND {} ({flags}) {{{}}}", quote(mailbox)?, raw.len());
    let id = s.run_command(&command).await?;
    loop {
        let Some(resp) = s.read_response().await? else {
            return Err(MailError::Connection("connection closed by server".into()));
        };
        match resp.parsed() {
            Response::Continue { .. } => break,
            Response::Done {
                tag,
                status,
                outcome,
            } if tag == &id => {
                completion("APPEND", status, outcome)?;
                return Err(MailError::rejected(
                    "APPEND",
                    "the server completed APPEND before accepting the message",
                ));
            }
            _ => continue,
        }
    }
    let stream = s.as_mut();
    stream.write_all(raw).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await?;
    read_done(s, "APPEND", &id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("Sent Messages").unwrap(), "\"Sent Messages\"");
        assert_eq!(quote("a\"b\\c").unwrap(), "\"a\\\"b\\\\c\"");
        assert!(quote("a\r\nb").is_err());
    }

    #[test]
    fn uid_sets() {
        assert_eq!(uid_set(&[3, 1, 2]), "3,1,2");
        assert_eq!(uid_set(&[7]), "7");
    }
}
