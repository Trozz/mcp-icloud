mod support;
use mcp_icloud_mail::cache::LruMessageCache;
use mcp_icloud_mail::error::MailError;
use mcp_icloud_mail::imap::{Connector, ImapClient, ImapSettings, SearchQuery};
use std::sync::Arc;
use std::time::Duration;
use support::fake_imap::{FakeImap, FakeState};

fn client(fake: &FakeImap) -> ImapClient {
    ImapClient::new(
        ImapSettings {
            connector: Connector::Plain {
                addr: fake.addr.clone(),
            },
            username: "me@icloud.com".into(),
            password: "app-pass".to_string().into(),
            idle_timeout: Duration::from_secs(600),
        },
        Arc::new(LruMessageCache::new(1024 * 1024)),
    )
}

fn msg(i: u32, from: &str, subject: &str) -> Vec<u8> {
    format!("From: {from}\r\nTo: me@icloud.com\r\nSubject: {subject}\r\nMessage-ID: <m{i}@example.com>\r\nDate: Wed, 15 Oct 2026 10:00:00 +0000\r\n\r\nbody {i}\r\n").into_bytes()
}

async fn inbox_with(n: u32) -> FakeImap {
    let mut st = FakeState::icloud_like();
    for i in 1..=n {
        let from = if i % 2 == 0 {
            "Alice <alice@example.com>"
        } else {
            "bob@example.com"
        };
        st.add_message(
            "INBOX",
            &msg(i, from, &format!("=?UTF-8?Q?Caf=C3=A9_{i}?=")),
            if i == 1 { &["\\Seen"] } else { &[] },
        );
    }
    FakeImap::start(st).await
}

#[tokio::test]
async fn search_returns_newest_first_with_decoded_summaries() {
    let fake = inbox_with(3).await;
    let page = client(&fake)
        .search("INBOX", &SearchQuery::default(), 25, None)
        .await
        .unwrap();
    assert_eq!(page.uidvalidity, 1000);
    assert_eq!(
        page.messages.iter().map(|m| m.uid).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
    let m2 = &page.messages[1];
    assert_eq!(m2.subject, "Caf\u{e9} 2");
    assert_eq!(m2.from, vec!["Alice <alice@example.com>"]);
    assert_eq!(m2.to, vec!["me@icloud.com"]);
    assert_eq!(m2.message_id.as_deref(), Some("m2@example.com"));
    assert!(
        m2.date
            .as_deref()
            .unwrap()
            .starts_with("2026-10-15T10:00:00")
    );
    assert_eq!(page.messages[2].flags, vec!["\\Seen"]);
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn search_filters_are_sent_to_the_server() {
    let fake = inbox_with(4).await;
    let q = SearchQuery {
        from: Some("alice".into()),
        unseen: Some(true),
        ..Default::default()
    };
    let page = client(&fake).search("INBOX", &q, 25, None).await.unwrap();
    assert_eq!(
        page.messages.iter().map(|m| m.uid).collect::<Vec<_>>(),
        vec![4, 2]
    );
    assert!(
        fake.log()
            .iter()
            .any(|l| l == "UID SEARCH FROM \"alice\" UNSEEN UNDELETED"),
        "{:?}",
        fake.log()
    );
}

#[tokio::test]
async fn pagination_walks_all_pages() {
    let fake = inbox_with(5).await;
    let c = client(&fake);
    let p1 = c
        .search("INBOX", &SearchQuery::default(), 2, None)
        .await
        .unwrap();
    assert_eq!(
        p1.messages.iter().map(|m| m.uid).collect::<Vec<_>>(),
        vec![5, 4]
    );
    let p2 = c
        .search(
            "INBOX",
            &SearchQuery::default(),
            2,
            p1.next_cursor.as_deref(),
        )
        .await
        .unwrap();
    assert_eq!(
        p2.messages.iter().map(|m| m.uid).collect::<Vec<_>>(),
        vec![3, 2]
    );
    let p3 = c
        .search(
            "INBOX",
            &SearchQuery::default(),
            2,
            p2.next_cursor.as_deref(),
        )
        .await
        .unwrap();
    assert_eq!(
        p3.messages.iter().map(|m| m.uid).collect::<Vec<_>>(),
        vec![1]
    );
    assert!(p3.next_cursor.is_none());
}

#[tokio::test]
async fn cursor_from_other_uidvalidity_is_rejected() {
    let fake = inbox_with(3).await;
    let c = client(&fake);
    let cursor = mcp_icloud_mail::imap::query::encode_cursor(999, 3);
    assert!(matches!(
        c.search("INBOX", &SearchQuery::default(), 2, Some(&cursor))
            .await,
        Err(MailError::UidValidityChanged { .. })
    ));
}

#[tokio::test]
async fn empty_mailbox_search() {
    let fake = inbox_with(0).await;
    let page = client(&fake)
        .search("INBOX", &SearchQuery::default(), 25, None)
        .await
        .unwrap();
    assert!(page.messages.is_empty());
}

#[tokio::test]
async fn tagged_no_on_fetch_is_an_error_not_an_empty_page() {
    let fake = inbox_with(2).await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("FETCH".into(), "simulated fetch failure".into());
    let err = client(&fake)
        .search("INBOX", &SearchQuery::default(), 25, None)
        .await
        .unwrap_err();
    assert!(matches!(err, MailError::Rejected { .. }), "{err}");
}

#[tokio::test]
async fn tagged_no_on_search_is_an_error_not_an_empty_page() {
    let fake = inbox_with(2).await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("SEARCH".into(), "simulated search failure".into());
    let err = client(&fake)
        .search("INBOX", &SearchQuery::default(), 25, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MailError::Rejected { info, .. } if info.contains("simulated search failure")),
        "{err}"
    );
}

#[tokio::test]
async fn fetch_raw_uses_cache_and_peek() {
    let fake = inbox_with(2).await;
    let c = client(&fake);
    let (uv, raw) = c.fetch_raw("INBOX", 2, None).await.unwrap();
    assert_eq!(uv, 1000);
    assert_eq!(
        raw.as_slice(),
        msg(2, "Alice <alice@example.com>", "=?UTF-8?Q?Caf=C3=A9_2?=").as_slice()
    );
    c.fetch_raw("INBOX", 2, Some(1000)).await.unwrap();
    let fetches = fake
        .log()
        .iter()
        .filter(|l| l.starts_with("UID FETCH 2 (UID BODY.PEEK[])"))
        .count();
    assert_eq!(fetches, 1, "second read must come from the cache");
    assert!(
        fake.messages("INBOX")[1].flags.is_empty(),
        "PEEK must not set \\Seen"
    );
}

#[tokio::test]
async fn fetch_raw_missing_uid() {
    let fake = inbox_with(1).await;
    assert!(matches!(
        client(&fake).fetch_raw("INBOX", 42, None).await,
        Err(MailError::MessageNotFound(42))
    ));
}

#[tokio::test]
async fn summaries_for_specific_uids() {
    let fake = inbox_with(3).await;
    let (uv, s) = client(&fake).summaries("INBOX", &[1, 3]).await.unwrap();
    assert_eq!(uv, 1000);
    assert_eq!(s.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![3, 1]);
}

#[tokio::test]
async fn has_attachments_for_multipart_mixed() {
    let mut st = FakeState::icloud_like();
    st.add_message("INBOX", b"From: a@example.com\r\nSubject: s\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nx\r\n--b--\r\n", &[]);
    st.add_message(
        "INBOX",
        b"From: a@example.com\r\nSubject: s\r\n\r\nx\r\n",
        &[],
    );
    let fake = FakeImap::start(st).await;
    let page = client(&fake)
        .search("INBOX", &SearchQuery::default(), 25, None)
        .await
        .unwrap();
    assert_eq!(
        page.messages
            .iter()
            .map(|m| m.has_attachments)
            .collect::<Vec<_>>(),
        vec![false, true]
    );
}
