mod support;
use futures::TryStreamExt;
use support::fake_imap::{FakeImap, FakeState};

const MSG: &[u8] = b"From: Alice <alice@example.com>\r\nTo: me@icloud.com\r\nSubject: =?UTF-8?Q?Caf=C3=A9?=\r\nMessage-ID: <m1@example.com>\r\n\r\nhello\r\n";

async fn session(fake: &FakeImap) -> async_imap::Session<tokio::net::TcpStream> {
    let tcp = tokio::net::TcpStream::connect(&fake.addr).await.unwrap();
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.unwrap().unwrap();
    client
        .login("me@icloud.com", "app-pass")
        .await
        .map_err(|(e, _)| e)
        .unwrap()
}

#[tokio::test]
async fn login_failure_is_no() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let tcp = tokio::net::TcpStream::connect(&fake.addr).await.unwrap();
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.unwrap().unwrap();
    let err = client
        .login("me@icloud.com", "wrong")
        .await
        .map_err(|(e, _)| e)
        .unwrap_err();
    assert!(err.to_string().contains("AUTHENTICATIONFAILED"), "{err}");
}

#[tokio::test]
async fn list_select_search_fetch_roundtrip() {
    let mut st = FakeState::icloud_like();
    st.add_message("INBOX", MSG, &["\\Seen"]);
    st.add_message(
        "INBOX",
        b"From: bob@example.com\r\nSubject: other\r\n\r\nx\r\n",
        &[],
    );
    let fake = FakeImap::start(st).await;
    let mut s = session(&fake).await;

    let names: Vec<_> = s
        .list(Some(""), Some("*"))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(names.len(), 6);
    assert!(names.iter().any(|n| {
        n.name() == "Sent Messages"
            && n.attributes()
                .contains(&async_imap::types::NameAttribute::Sent)
    }));

    let mb = s.select("INBOX").await.unwrap();
    assert_eq!(mb.exists, 2);
    assert_eq!(mb.uid_validity, Some(1000));

    let unseen = s.uid_search("UNSEEN").await.unwrap();
    assert_eq!(unseen.into_iter().collect::<Vec<_>>(), vec![2]);
    let from = s.uid_search("FROM \"alice\"").await.unwrap();
    assert_eq!(from.into_iter().collect::<Vec<_>>(), vec![1]);

    let fetches: Vec<_> = s
        .uid_fetch("1", "(UID FLAGS ENVELOPE BODY.PEEK[])")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0].uid, Some(1));
    assert_eq!(fetches[0].body().unwrap(), MSG);
    let env = fetches[0].envelope().unwrap();
    assert_eq!(env.subject.as_deref(), Some(&b"=?UTF-8?Q?Caf=C3=A9?="[..]));
    assert_eq!(env.message_id.as_deref(), Some(&b"<m1@example.com>"[..]));
}

#[tokio::test]
async fn injected_failure_is_one_shot() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("SELECT".into(), "simulated failure".into());
    let mut s = session(&fake).await;
    assert!(s.select("INBOX").await.is_err());
    assert!(s.select("INBOX").await.is_ok());
}

#[tokio::test]
async fn store_and_move_change_state() {
    let mut st = FakeState::icloud_like();
    st.add_message("INBOX", MSG, &[]);
    let fake = FakeImap::start(st).await;
    let mut s = session(&fake).await;
    s.select("INBOX").await.unwrap();
    s.run_command_and_check_ok("UID STORE 1 +FLAGS.SILENT (\\Flagged)")
        .await
        .unwrap();
    assert_eq!(fake.messages("INBOX")[0].flags, vec!["\\Flagged"]);
    s.run_command_and_check_ok("UID MOVE 1 \"Archive\"")
        .await
        .unwrap();
    assert!(fake.messages("INBOX").is_empty());
    assert_eq!(fake.messages("Archive").len(), 1);
    assert!(fake.log().iter().any(|l| l.starts_with("UID MOVE")));
}
