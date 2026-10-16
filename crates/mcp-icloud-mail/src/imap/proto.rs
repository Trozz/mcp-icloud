//! Low-level helpers over an authenticated async-imap session.
//!
//! async-imap ends streaming commands (UID FETCH, UID STORE, LIST, UID EXPUNGE) silently when the
//! server answers with a tagged NO/BAD, so every mutating command is sent with `run_checked`,
//! which reads responses up to the tagged completion and checks its status.

use crate::error::{MailError, Result};
use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::imap_proto::{Response, ResponseCode, Status};
use tokio::io::{AsyncRead, AsyncWrite};

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

/// Reads responses until the tagged completion and maps NO/BAD to `MailError::Rejected`.
/// Commands are never pipelined, so the first tagged completion belongs to the last command.
pub async fn read_done(s: &mut ImapSession, label: &str) -> Result<Done> {
    loop {
        let Some(resp) = s.read_response().await? else {
            return Err(MailError::Connection("connection closed by server".into()));
        };
        let Response::Done {
            status, outcome, ..
        } = resp.parsed()
        else {
            continue;
        };
        return match status {
            Status::Ok => Ok(Done {
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
            }),
            _ => Err(MailError::rejected(
                label,
                outcome
                    .information
                    .as_deref()
                    .unwrap_or("no details")
                    .to_string(),
            )),
        };
    }
}

pub async fn run_checked(s: &mut ImapSession, label: &str, command: &str) -> Result<Done> {
    s.run_command(command).await?;
    read_done(s, label).await
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
