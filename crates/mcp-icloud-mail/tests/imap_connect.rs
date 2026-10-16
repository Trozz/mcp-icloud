mod support;
use mcp_icloud_mail::cache::LruMessageCache;
use mcp_icloud_mail::error::MailError;
use mcp_icloud_mail::imap::{Connector, ImapClient, ImapSettings, Role};
use std::sync::Arc;
use std::time::Duration;
use support::fake_imap::{FakeImap, FakeState};

fn client(fake: &FakeImap, password: &str) -> ImapClient {
    ImapClient::new(
        ImapSettings {
            connector: Connector::Plain {
                addr: fake.addr.clone(),
            },
            username: "me@icloud.com".into(),
            password: password.to_string().into(),
            idle_timeout: Duration::from_secs(600),
        },
        Arc::new(LruMessageCache::new(1024 * 1024)),
    )
}

#[tokio::test]
async fn wrong_password_is_auth_failed() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let err = client(&fake, "wrong").mailboxes().await.unwrap_err();
    assert!(matches!(err, MailError::AuthFailed), "{err}");
}

#[tokio::test]
async fn lists_mailboxes_with_roles_and_counts() {
    let mut st = FakeState::icloud_like();
    st.add_message("INBOX", b"Subject: a\r\n\r\nx", &["\\Seen"]);
    st.add_message("INBOX", b"Subject: b\r\n\r\nx", &[]);
    let fake = FakeImap::start(st).await;
    let c = client(&fake, "app-pass");
    let boxes = c.mailboxes().await.unwrap();
    let inbox = boxes.iter().find(|b| b.name == "INBOX").unwrap();
    assert_eq!(
        (inbox.total, inbox.unseen, inbox.role),
        (2, 1, Some(Role::Inbox))
    );
    assert_eq!(
        boxes
            .iter()
            .find(|b| b.name == "Deleted Messages")
            .unwrap()
            .role,
        Some(Role::Trash)
    );
    assert_eq!(c.role_mailbox(Role::Drafts).await.unwrap(), "Drafts");
    assert_eq!(c.role_mailbox(Role::Sent).await.unwrap(), "Sent Messages");
}

#[tokio::test]
async fn capabilities_are_detected() {
    let mut st = FakeState::icloud_like();
    st.capabilities = vec![];
    let fake = FakeImap::start(st).await;
    let caps = client(&fake, "app-pass").capabilities().await.unwrap();
    assert!(!caps.move_ && !caps.uidplus);
}

#[tokio::test]
async fn uidvalidity_mismatch_and_missing_mailbox() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let c = client(&fake, "app-pass");
    assert_eq!(c.uidvalidity("INBOX", None).await.unwrap(), 1000);
    assert!(matches!(
        c.uidvalidity("INBOX", Some(999)).await,
        Err(MailError::UidValidityChanged {
            expected: 999,
            actual: 1000,
            ..
        })
    ));
    assert!(matches!(
        c.uidvalidity("Nope", None).await,
        Err(MailError::MailboxNotFound(_))
    ));
}

#[tokio::test]
async fn connection_is_reused() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let c = client(&fake, "app-pass");
    c.uidvalidity("INBOX", None).await.unwrap();
    c.uidvalidity("INBOX", None).await.unwrap();
    let logins = fake.log().iter().filter(|l| l.starts_with("LOGIN")).count();
    assert_eq!(logins, 1);
}

#[tokio::test]
async fn read_operation_reconnects_after_dropped_connection() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let c = client(&fake, "app-pass");
    c.uidvalidity("INBOX", None).await.unwrap();
    fake.state.lock().unwrap().drop_on = Some("SELECT".into());
    assert_eq!(c.uidvalidity("INBOX", None).await.unwrap(), 1000);
    let logins = fake.log().iter().filter(|l| l.starts_with("LOGIN")).count();
    assert_eq!(logins, 2);
}

#[tokio::test]
async fn idle_timeout_reconnects() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let mut settings = ImapSettings {
        connector: Connector::Plain {
            addr: fake.addr.clone(),
        },
        username: "me@icloud.com".into(),
        password: "app-pass".to_string().into(),
        idle_timeout: Duration::from_millis(10),
    };
    settings.idle_timeout = Duration::from_millis(10);
    let c = ImapClient::new(settings, Arc::new(LruMessageCache::new(0)));
    c.uidvalidity("INBOX", None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    c.uidvalidity("INBOX", None).await.unwrap();
    assert_eq!(
        fake.log().iter().filter(|l| l.starts_with("LOGIN")).count(),
        2
    );
}
