mod support;
use mcp_icloud_mail::cache::{CacheKey, LruMessageCache, MessageCache};
use mcp_icloud_mail::error::MailError;
use mcp_icloud_mail::imap::{
    COMMAND_TIMEOUT, CONNECT_TIMEOUT, Connector, ImapClient, ImapSettings,
};
use std::sync::Arc;
use std::time::Duration;
use support::fake_imap::{FakeImap, FakeMailbox, FakeState};

fn client(fake: &FakeImap) -> ImapClient {
    ImapClient::new(
        ImapSettings {
            connector: Connector::Plain {
                addr: fake.addr.clone(),
            },
            username: "me@icloud.com".into(),
            password: "app-pass".to_string().into(),
            idle_timeout: Duration::from_secs(600),
            connect_timeout: CONNECT_TIMEOUT,
            command_timeout: COMMAND_TIMEOUT,
        },
        Arc::new(LruMessageCache::new(1024 * 1024)),
    )
}

fn msg(i: u32) -> Vec<u8> {
    format!(
        "From: a@example.com\r\nSubject: m{i}\r\nMessage-ID: <m{i}@example.com>\r\n\r\nbody {i}\r\n"
    )
    .into_bytes()
}

async fn fake_with(caps: &[&str], inbox: u32, trash: u32) -> FakeImap {
    let mut st = FakeState::icloud_like();
    st.capabilities = caps.iter().map(|c| c.to_string()).collect();
    for i in 1..=inbox {
        st.add_message("INBOX", &msg(i), &[]);
    }
    for i in 1..=trash {
        st.add_message("Deleted Messages", &msg(100 + i), &["\\Seen"]);
    }
    FakeImap::start(st).await
}

fn uids(fake: &FakeImap, mailbox: &str) -> Vec<u32> {
    fake.messages(mailbox).iter().map(|m| m.uid).collect()
}

#[tokio::test]
async fn set_flags_adds_and_removes() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 2, 0).await;
    let c = client(&fake);
    c.set_flags("INBOX", &[1, 2], Some(1000), "\\Seen", true)
        .await
        .unwrap();
    assert!(
        fake.messages("INBOX")
            .iter()
            .all(|m| m.flags == vec!["\\Seen"])
    );
    c.set_flags("INBOX", &[2], None, "\\Seen", false)
        .await
        .unwrap();
    assert!(fake.messages("INBOX")[1].flags.is_empty());
    assert!(
        fake.log()
            .iter()
            .any(|l| l == "UID STORE 1,2 +FLAGS.SILENT (\\Seen)")
    );
}

#[tokio::test]
async fn tagged_no_on_store_is_an_error() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 0).await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("STORE".into(), "simulated store failure".into());
    let err = client(&fake)
        .set_flags("INBOX", &[1], None, "\\Flagged", true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MailError::Rejected { info, .. } if info.contains("simulated store failure")),
        "{err}"
    );
    assert!(fake.messages("INBOX")[0].flags.is_empty());
}

#[tokio::test]
async fn completion_with_another_tag_is_not_taken_as_ours() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 0).await;
    {
        let mut st = fake.state.lock().unwrap();
        st.stale_tag_on = Some("STORE".into());
        st.fail
            .insert("STORE".into(), "simulated store failure".into());
    }
    let err = client(&fake)
        .set_flags("INBOX", &[1], None, "\\Flagged", true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MailError::Rejected { info, .. } if info.contains("simulated store failure")),
        "{err}"
    );
}

#[tokio::test]
async fn stale_uidvalidity_leaves_mailbox_untouched() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 2, 0).await;
    let c = client(&fake);
    let flags = c.set_flags("INBOX", &[1], Some(999), "\\Seen", true).await;
    let moved = c.move_messages("INBOX", &[1], "Archive", Some(999)).await;
    let deleted = c.delete_messages("INBOX", &[2], false, Some(999)).await;
    for r in [flags, moved, deleted] {
        assert!(
            matches!(r, Err(MailError::UidValidityChanged { .. })),
            "{r:?}"
        );
    }
    assert_eq!(uids(&fake, "INBOX"), vec![1, 2]);
    assert!(fake.messages("INBOX").iter().all(|m| m.flags.is_empty()));
    assert!(fake.messages("Archive").is_empty());
}

#[tokio::test]
async fn rejects_invalid_uid_lists_and_flags() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 0).await;
    let c = client(&fake);
    assert!(matches!(
        c.set_flags("INBOX", &[], None, "\\Seen", true).await,
        Err(MailError::Invalid(_))
    ));
    let many: Vec<u32> = (1..=101).collect();
    assert!(matches!(
        c.move_messages("INBOX", &many, "Archive", None).await,
        Err(MailError::Invalid(_))
    ));
    assert!(matches!(
        c.set_flags("INBOX", &[1], None, "\\Deleted", true).await,
        Err(MailError::Invalid(_))
    ));
}

#[tokio::test]
async fn move_uses_move_when_available() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 2, 0).await;
    client(&fake)
        .move_messages("INBOX", &[1], "Archive", Some(1000))
        .await
        .unwrap();
    assert_eq!(uids(&fake, "INBOX"), vec![2]);
    assert_eq!(fake.messages("Archive").len(), 1);
    assert!(fake.log().iter().any(|l| l == "UID MOVE 1 \"Archive\""));
}

fn cached_client(fake: &FakeImap) -> (ImapClient, Arc<LruMessageCache>) {
    let cache = Arc::new(LruMessageCache::new(1024 * 1024));
    let c = ImapClient::new(
        ImapSettings {
            connector: Connector::Plain {
                addr: fake.addr.clone(),
            },
            username: "me@icloud.com".into(),
            password: "app-pass".to_string().into(),
            idle_timeout: Duration::from_secs(600),
            connect_timeout: CONNECT_TIMEOUT,
            command_timeout: COMMAND_TIMEOUT,
        },
        cache.clone(),
    );
    (c, cache)
}

fn key(mailbox: &str, uid: u32) -> CacheKey {
    CacheKey {
        mailbox: mailbox.into(),
        uidvalidity: 1000,
        uid,
    }
}

#[tokio::test]
async fn moved_and_deleted_messages_leave_the_cache() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 2, 1).await;
    let (c, cache) = cached_client(&fake);
    c.fetch_raw("INBOX", 1, None).await.unwrap();
    c.fetch_raw("INBOX", 2, None).await.unwrap();
    c.fetch_raw("Deleted Messages", 1, None).await.unwrap();
    c.move_messages("INBOX", &[1], "Archive", None)
        .await
        .unwrap();
    assert!(cache.get(&key("INBOX", 1)).is_none());
    c.delete_messages("INBOX", &[2], false, None).await.unwrap();
    assert!(cache.get(&key("INBOX", 2)).is_none());
    c.delete_messages("Deleted Messages", &[1], true, None)
        .await
        .unwrap();
    assert!(cache.get(&key("Deleted Messages", 1)).is_none());
}

#[tokio::test]
async fn move_falls_back_to_copy_store_expunge_without_move() {
    let fake = fake_with(&["UIDPLUS"], 2, 0).await;
    client(&fake)
        .move_messages("INBOX", &[2], "Archive", None)
        .await
        .unwrap();
    assert_eq!(uids(&fake, "INBOX"), vec![1]);
    assert_eq!(fake.messages("Archive").len(), 1);
    let log = fake.log();
    assert!(log.iter().any(|l| l == "UID COPY 2 \"Archive\""), "{log:?}");
    assert!(log.iter().any(|l| l == "UID EXPUNGE 2"), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("UID MOVE")));
}

#[tokio::test]
async fn move_without_move_or_uidplus_is_unsupported() {
    let fake = fake_with(&[], 2, 0).await;
    let err = client(&fake)
        .move_messages("INBOX", &[1], "Archive", None)
        .await
        .unwrap_err();
    assert!(matches!(err, MailError::Unsupported(_)), "{err}");
    assert_eq!(uids(&fake, "INBOX"), vec![1, 2]);
}

#[tokio::test]
async fn move_to_unknown_mailbox() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 0).await;
    assert!(matches!(
        client(&fake)
            .move_messages("INBOX", &[1], "Nope", None)
            .await,
        Err(MailError::MailboxNotFound(_))
    ));
}

#[tokio::test]
async fn move_to_a_mailbox_created_elsewhere_refreshes_the_list() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 0).await;
    let c = client(&fake);
    c.uidvalidity("INBOX", None).await.unwrap();
    fake.state.lock().unwrap().mailboxes.insert(
        "Projects".into(),
        FakeMailbox {
            attrs: vec![],
            uidvalidity: 3000,
            uidnext: 1,
            messages: vec![],
        },
    );
    c.move_messages("INBOX", &[1], "Projects", None)
        .await
        .unwrap();
    assert_eq!(uids(&fake, "Projects"), vec![1]);
}

#[tokio::test]
async fn delete_moves_to_trash() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 2, 0).await;
    client(&fake)
        .delete_messages("INBOX", &[1, 2], false, None)
        .await
        .unwrap();
    assert!(fake.messages("INBOX").is_empty());
    assert_eq!(fake.messages("Deleted Messages").len(), 2);
}

#[tokio::test]
async fn delete_rules_for_trash_and_permanent() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 2).await;
    let c = client(&fake);
    assert!(matches!(
        c.delete_messages("Deleted Messages", &[1], false, None)
            .await,
        Err(MailError::Invalid(_))
    ));
    assert!(matches!(
        c.delete_messages("INBOX", &[1], true, None).await,
        Err(MailError::Invalid(_))
    ));
    c.delete_messages("Deleted Messages", &[1], true, Some(1000))
        .await
        .unwrap();
    assert_eq!(uids(&fake, "Deleted Messages"), vec![2]);
    assert_eq!(uids(&fake, "INBOX"), vec![1]);
}

#[tokio::test]
async fn permanent_delete_without_uidplus_is_refused() {
    let fake = fake_with(&["MOVE"], 0, 2).await;
    let err = client(&fake)
        .delete_messages("Deleted Messages", &[1], true, None)
        .await
        .unwrap_err();
    assert!(matches!(err, MailError::Unsupported(_)), "{err}");
    assert_eq!(uids(&fake, "Deleted Messages"), vec![1, 2]);
    assert!(!fake.log().iter().any(|l| l.contains("EXPUNGE")));
}

#[tokio::test]
async fn create_mailbox_refreshes_the_list() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 1, 0).await;
    let c = client(&fake);
    c.mailboxes().await.unwrap();
    c.create_mailbox("Projects").await.unwrap();
    c.move_messages("INBOX", &[1], "Projects", None)
        .await
        .unwrap();
    assert_eq!(fake.messages("Projects").len(), 1);
    assert!(matches!(
        c.create_mailbox("Projects").await,
        Err(MailError::Rejected { .. })
    ));
}

fn drafts_uidvalidity(fake: &FakeImap, uv: u32) {
    let mut st = fake.state.lock().unwrap();
    st.mailboxes.get_mut("Drafts").unwrap().uidvalidity = uv;
}

#[tokio::test]
async fn append_returns_appenduid() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 0, 0).await;
    drafts_uidvalidity(&fake, 1234);
    let appended = client(&fake)
        .append("Drafts", "\\Draft \\Seen", &msg(7), "m7@example.com")
        .await
        .unwrap();
    assert_eq!(appended, (1234, Some(1)));
    assert!(!fake.log().iter().any(|l| l.starts_with("SELECT")));
    let drafts = fake.messages("Drafts");
    assert_eq!(drafts[0].raw, msg(7));
    assert_eq!(drafts[0].flags, vec!["\\Draft", "\\Seen"]);
}

#[tokio::test]
async fn append_without_uidplus_finds_uid_by_message_id() {
    let fake = fake_with(&["MOVE"], 0, 0).await;
    fake.state
        .lock()
        .unwrap()
        .add_message("Drafts", &msg(1), &[]);
    drafts_uidvalidity(&fake, 1234);
    let appended = client(&fake)
        .append("Drafts", "\\Draft", &msg(2), "m2@example.com")
        .await
        .unwrap();
    assert_eq!(appended, (1234, Some(2)));
    assert!(
        fake.log()
            .iter()
            .any(|l| l == "UID SEARCH HEADER Message-ID \"<m2@example.com>\"")
    );
}

#[tokio::test]
async fn append_rejected_is_an_error() {
    let fake = fake_with(&["MOVE", "UIDPLUS"], 0, 0).await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("APPEND".into(), "over quota".into());
    let err = client(&fake)
        .append("Drafts", "\\Draft", &msg(1), "m1@example.com")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MailError::Rejected { info, .. } if info == "over quota"),
        "{err}"
    );
    assert!(fake.messages("Drafts").is_empty());
}
