use super::client::{Conn, ImapClient, select, with_conn};
use super::proto::{uid_search, uid_set};
use super::query::{SearchQuery, build_search, decode_cursor, encode_cursor};
use super::types::{MessageSummary, SearchPage};
use crate::cache::CacheKey;
use crate::error::{MailError, Result};
use crate::mail::parse::decode_header_words;
use async_imap::imap_proto::{Address, BodyStructure};
use async_imap::types::{Fetch, Flag};
use futures::TryStreamExt;
use std::sync::Arc;

const SUMMARY_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE ENVELOPE BODYSTRUCTURE)";

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn format_address(a: &Address<'_>) -> Option<String> {
    let mailbox = lossy(a.mailbox.as_deref()?);
    let email = match a.host.as_deref() {
        Some(h) if !h.is_empty() => format!("{mailbox}@{}", lossy(h)),
        _ => mailbox,
    };
    Some(match a.name.as_deref() {
        Some(n) if !n.is_empty() => format!("{} <{email}>", decode_header_words(n)),
        _ => email,
    })
}

fn addresses(list: Option<&Vec<Address<'_>>>) -> Vec<String> {
    list.map(|v| v.iter().filter_map(format_address).collect())
        .unwrap_or_default()
}

pub(crate) fn flag_name(f: &Flag<'_>) -> String {
    match f {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(c) => c.to_string(),
    }
}

/// Heuristic: a top-level multipart/mixed message almost always carries attachments.
fn has_attachments(bs: Option<&BodyStructure<'_>>) -> bool {
    matches!(bs, Some(BodyStructure::Multipart { common, .. }) if common.ty.subtype.eq_ignore_ascii_case("mixed"))
}

pub(crate) fn summary(f: &Fetch) -> Option<MessageSummary> {
    let uid = f.uid?;
    let env = f.envelope();
    Some(MessageSummary {
        uid,
        date: f.internal_date().map(|d| d.to_rfc3339()),
        from: addresses(env.and_then(|e| e.from.as_ref())),
        to: addresses(env.and_then(|e| e.to.as_ref())),
        subject: env
            .and_then(|e| e.subject.as_deref())
            .map(decode_header_words)
            .unwrap_or_default(),
        message_id: env.and_then(|e| e.message_id.as_deref()).map(|m| {
            lossy(m)
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        }),
        flags: f.flags().map(|fl| flag_name(&fl)).collect(),
        size: f.size.unwrap_or(0),
        has_attachments: has_attachments(f.bodystructure()),
    })
}

pub(crate) async fn search_uids(c: &mut Conn, criteria: &str) -> Result<Vec<u32>> {
    let mut uids = uid_search(&mut c.session, criteria).await?;
    uids.sort_unstable_by_key(|u| std::cmp::Reverse(*u));
    uids.dedup();
    Ok(uids)
}

pub(crate) async fn summaries(c: &mut Conn, uids: &[u32]) -> Result<Vec<MessageSummary>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let fetches: Vec<Fetch> = c
        .session
        .uid_fetch(uid_set(uids), SUMMARY_ITEMS)
        .await?
        .try_collect()
        .await?;
    // async-imap ends the stream silently on a tagged NO; an empty answer for existing uids is a failure.
    if fetches.is_empty() {
        return Err(MailError::rejected(
            "UID FETCH",
            "the server returned no messages",
        ));
    }
    let mut out: Vec<MessageSummary> = fetches.iter().filter_map(summary).collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.uid));
    Ok(out)
}

pub(crate) async fn fetch_body(c: &mut Conn, uid: u32) -> Result<Vec<u8>> {
    let fetches: Vec<Fetch> = c
        .session
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
        .await?
        .try_collect()
        .await?;
    fetches
        .iter()
        .find(|f| f.uid == Some(uid))
        .and_then(|f| f.body())
        .map(<[u8]>::to_vec)
        .ok_or(MailError::MessageNotFound(uid))
}

impl ImapClient {
    /// One page of messages matching `q`, newest first.
    pub async fn search(
        &self,
        mailbox: &str,
        q: &SearchQuery,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<SearchPage> {
        let (expected_uv, below) = match cursor {
            Some(c) => {
                let (uv, uid) = decode_cursor(c)?;
                (Some(uv), Some(uid))
            }
            None => (None, None),
        };
        let criteria = build_search(q, below)?;
        let limit = limit.clamp(1, 100);
        with_conn!(self, "UID SEARCH", true, |c| async {
            let uidvalidity = select(c, mailbox, expected_uv).await?;
            let Some(criteria) = &criteria else {
                return Ok(SearchPage {
                    mailbox: mailbox.to_string(),
                    uidvalidity,
                    messages: vec![],
                    next_cursor: None,
                });
            };
            let uids = search_uids(c, criteria).await?;
            let page: Vec<u32> = uids.iter().copied().take(limit).collect();
            let next_cursor = (uids.len() > limit)
                .then(|| encode_cursor(uidvalidity, *page.last().expect("non-empty page")));
            let messages = summaries(c, &page).await?;
            Ok::<_, MailError>(SearchPage {
                mailbox: mailbox.to_string(),
                uidvalidity,
                messages,
                next_cursor,
            })
        })
    }

    /// Summaries for specific uids (missing uids are skipped). Returns the mailbox UIDVALIDITY too.
    pub async fn summaries(
        &self,
        mailbox: &str,
        uids: &[u32],
    ) -> Result<(u32, Vec<MessageSummary>)> {
        with_conn!(self, "UID FETCH", true, |c| async {
            let uv = select(c, mailbox, None).await?;
            let s = summaries(c, uids).await?;
            Ok::<_, MailError>((uv, s))
        })
    }

    /// Raw UIDs matching IMAP search criteria, newest first.
    pub async fn search_uids(&self, mailbox: &str, criteria: &str) -> Result<Vec<u32>> {
        with_conn!(self, "UID SEARCH", true, |c| async {
            select(c, mailbox, None).await?;
            search_uids(c, criteria).await
        })
    }

    /// The full RFC 822 source of a message, served from the cache when possible.
    /// Uses BODY.PEEK[] so the message is not marked as read.
    pub async fn fetch_raw(
        &self,
        mailbox: &str,
        uid: u32,
        expected_uv: Option<u32>,
    ) -> Result<(u32, Arc<Vec<u8>>)> {
        with_conn!(self, "UID FETCH", true, |c| async {
            let uidvalidity = select(c, mailbox, expected_uv).await?;
            let key = CacheKey {
                mailbox: mailbox.to_string(),
                uidvalidity,
                uid,
            };
            if let Some(raw) = self.cache.get(&key) {
                return Ok((uidvalidity, raw));
            }
            let raw = Arc::new(fetch_body(c, uid).await?);
            self.cache.put(key, raw.clone());
            Ok::<_, MailError>((uidvalidity, raw))
        })
    }
}
