use super::client::{Conn, ImapClient, list_mailboxes, select, with_conn};
use super::proto::{append_literal, quote, run_checked, uid_set};
use super::read::search_uids;
use super::types::Role;
use crate::cache::CacheKey;
use crate::error::{MailError, Result};

const MAX_UIDS: usize = 100;
const ALLOWED_FLAGS: &[&str] = &["\\Seen", "\\Flagged", "\\Answered", "\\Draft"];

fn check_uids(uids: &[u32]) -> Result<String> {
    if uids.is_empty() || uids.len() > MAX_UIDS {
        return Err(MailError::Invalid(format!(
            "uids must contain between 1 and {MAX_UIDS} entries"
        )));
    }
    if uids.contains(&0) {
        return Err(MailError::Invalid(
            "uid 0 is not a valid message uid".into(),
        ));
    }
    Ok(uid_set(uids))
}

/// Checks `name` against the mailbox list, refreshing the list once on a miss so mailboxes
/// created by another client are found.
async fn known_mailbox(c: &mut Conn, name: &str) -> Result<()> {
    if c.mailboxes.iter().any(|m| m.name == name) {
        return Ok(());
    }
    c.mailboxes = list_mailboxes(&mut c.session).await?;
    if c.mailboxes.iter().any(|m| m.name == name) {
        return Ok(());
    }
    Err(MailError::MailboxNotFound(name.to_string()))
}

fn trash(c: &Conn) -> Result<String> {
    c.role(Role::Trash)
        .map(str::to_string)
        .ok_or_else(|| MailError::MailboxNotFound("Trash mailbox".into()))
}

/// Moves the uids in the currently selected mailbox to `target`. Uses MOVE when available,
/// otherwise COPY + \Deleted + UID EXPUNGE, which needs UIDPLUS so only these uids are expunged.
async fn move_selected(c: &mut Conn, set: &str, target: &str) -> Result<()> {
    let target = quote(target)?;
    if c.caps.move_ {
        run_checked(
            &mut c.session,
            "UID MOVE",
            &format!("UID MOVE {set} {target}"),
        )
        .await?;
        return Ok(());
    }
    if !c.caps.uidplus {
        return Err(MailError::Unsupported(
            "the server supports neither MOVE nor UIDPLUS, so messages cannot be moved without risking other deleted messages".into(),
        ));
    }
    run_checked(
        &mut c.session,
        "UID COPY",
        &format!("UID COPY {set} {target}"),
    )
    .await?;
    run_checked(
        &mut c.session,
        "UID STORE",
        &format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"),
    )
    .await?;
    run_checked(&mut c.session, "UID EXPUNGE", &format!("UID EXPUNGE {set}")).await?;
    Ok(())
}

impl ImapClient {
    /// Drops cached bodies of messages about to be moved or expunged from `mailbox`.
    fn forget(&self, mailbox: &str, uidvalidity: u32, uids: &[u32]) {
        for &uid in uids {
            self.cache.remove(&CacheKey {
                mailbox: mailbox.to_string(),
                uidvalidity,
                uid,
            });
        }
    }

    /// Adds or removes one system flag (`\Seen`, `\Flagged`, `\Answered`, `\Draft`).
    pub async fn set_flags(
        &self,
        mailbox: &str,
        uids: &[u32],
        uidvalidity: Option<u32>,
        flag: &str,
        add: bool,
    ) -> Result<()> {
        let set = check_uids(uids)?;
        if !ALLOWED_FLAGS.contains(&flag) {
            return Err(MailError::Invalid(format!(
                "flag must be one of {}",
                ALLOWED_FLAGS.join(", ")
            )));
        }
        let op = if add { '+' } else { '-' };
        with_conn!(self, "UID STORE", false, |c| async {
            select(c, mailbox, uidvalidity).await?;
            run_checked(
                &mut c.session,
                "UID STORE",
                &format!("UID STORE {set} {op}FLAGS.SILENT ({flag})"),
            )
            .await?;
            Ok::<_, MailError>(())
        })
    }

    pub async fn move_messages(
        &self,
        mailbox: &str,
        uids: &[u32],
        target: &str,
        uidvalidity: Option<u32>,
    ) -> Result<()> {
        let set = check_uids(uids)?;
        if mailbox == target {
            return Err(MailError::Invalid(
                "target_mailbox is the same as mailbox".into(),
            ));
        }
        with_conn!(self, "UID MOVE", false, |c| async {
            known_mailbox(c, target).await?;
            let uv = select(c, mailbox, uidvalidity).await?;
            self.forget(mailbox, uv, uids);
            move_selected(c, &set, target).await
        })
    }

    /// Moves messages to Trash, or with `permanent` expunges messages that are already in Trash.
    pub async fn delete_messages(
        &self,
        mailbox: &str,
        uids: &[u32],
        permanent: bool,
        uidvalidity: Option<u32>,
    ) -> Result<()> {
        let set = check_uids(uids)?;
        with_conn!(self, "DELETE_MESSAGES", false, |c| async {
            let trash = trash(c)?;
            if !permanent {
                if mailbox == trash {
                    return Err(MailError::Invalid(format!(
                        "the messages are already in {trash}; use permanent: true to remove them for good"
                    )));
                }
                let uv = select(c, mailbox, uidvalidity).await?;
                self.forget(mailbox, uv, uids);
                return move_selected(c, &set, &trash).await;
            }
            if mailbox != trash {
                return Err(MailError::Invalid(format!(
                    "permanent delete is only allowed in {trash}; delete without permanent first"
                )));
            }
            if !c.caps.uidplus {
                return Err(MailError::Unsupported(
                    "permanent delete needs the UIDPLUS extension so that only these messages are expunged".into(),
                ));
            }
            let uv = select(c, mailbox, uidvalidity).await?;
            self.forget(mailbox, uv, uids);
            run_checked(
                &mut c.session,
                "UID STORE",
                &format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"),
            )
            .await?;
            run_checked(&mut c.session, "UID EXPUNGE", &format!("UID EXPUNGE {set}")).await?;
            Ok::<_, MailError>(())
        })
    }

    pub async fn create_mailbox(&self, name: &str) -> Result<()> {
        let quoted = quote(name)?;
        if name.trim().is_empty() {
            return Err(MailError::Invalid("mailbox name must not be empty".into()));
        }
        with_conn!(self, "CREATE", false, |c| async {
            run_checked(&mut c.session, "CREATE", &format!("CREATE {quoted}")).await?;
            c.mailboxes = list_mailboxes(&mut c.session).await?;
            Ok::<_, MailError>(())
        })
    }

    /// Appends a message and returns its uid: from APPENDUID when the server sends it, otherwise
    /// by searching for its Message-ID (given without angle brackets).
    pub async fn append(
        &self,
        mailbox: &str,
        flags: &str,
        raw: &[u8],
        message_id: &str,
    ) -> Result<Option<u32>> {
        let header = quote(&format!("<{message_id}>"))?;
        with_conn!(self, "APPEND", false, |c| async {
            let done = append_literal(&mut c.session, mailbox, flags, raw).await?;
            if let Some(uid) = done.append_uid {
                return Ok(Some(uid));
            }
            select(c, mailbox, None).await?;
            let uids = search_uids(c, &format!("HEADER Message-ID {header}")).await?;
            Ok::<_, MailError>(uids.first().copied())
        })
    }
}
