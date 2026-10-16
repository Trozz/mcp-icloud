mod support;
use mcp_icloud_mail::smtp::{RecordingSender, Sender};
use mcp_icloud_mail::tools::MailServer;
use serde_json::json;
use std::sync::Arc;
use support::fake_imap::{FakeImap, FakeState};
use support::tools::{ME, is_error, json, params, server, text};

struct Setup {
    fake: FakeImap,
    sender: Arc<RecordingSender>,
    server: MailServer,
    draft_uid: u64,
}

async fn setup() -> Setup {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let sender = Arc::new(RecordingSender::default());
    let server = server(&fake, Some(sender.clone() as Arc<dyn Sender>));
    let v = json(
        &server
            .create_draft(params(json!({
                "to": ["Bob <bob@example.com>"],
                "cc": ["carol@example.com"],
                "bcc": ["secret@example.com"],
                "subject": "Plans",
                "body_text": "See you there."
            })))
            .await
            .unwrap(),
    );
    let draft_uid = v["uid"].as_u64().unwrap();
    Setup {
        fake,
        sender,
        server,
        draft_uid,
    }
}

fn has_bcc_header(raw: &[u8]) -> bool {
    String::from_utf8_lossy(raw)
        .lines()
        .take_while(|l| !l.is_empty())
        .any(|l| l.to_ascii_lowercase().starts_with("bcc:"))
}

#[tokio::test]
async fn send_draft_is_only_registered_with_a_sender() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    assert!(
        !server(&fake, None)
            .tool_names()
            .contains(&"send_draft".to_string())
    );
    let with = server(&fake, Some(Arc::new(RecordingSender::default())));
    let names = with.tool_names();
    assert!(names.contains(&"send_draft".to_string()));
    assert_eq!(names.len(), 13);
}

#[tokio::test]
async fn sends_without_bcc_header_and_files_the_message() {
    let s = setup().await;
    let v = json(
        &s.server
            .send_draft(params(json!({"uid": s.draft_uid, "uidvalidity": 1000})))
            .await
            .unwrap(),
    );
    assert_eq!(v["sent"], true);
    assert_eq!(v["saved_to"], "Sent Messages");

    let sent = s.sender.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from, ME);
    let mut rcpt = sent[0].recipients.clone();
    rcpt.sort();
    assert_eq!(
        rcpt,
        vec!["bob@example.com", "carol@example.com", "secret@example.com"]
    );
    assert!(!has_bcc_header(&sent[0].raw), "Bcc must not reach SMTP");
    assert!(!String::from_utf8_lossy(&sent[0].raw).contains("secret@example.com"));

    let sent_box = s.fake.messages("Sent Messages");
    assert_eq!(sent_box.len(), 1);
    assert_eq!(sent_box[0].flags, vec!["\\Seen"]);
    assert!(
        has_bcc_header(&sent_box[0].raw),
        "the Sent copy keeps Bcc for the owner"
    );
    assert!(s.fake.messages("Drafts").is_empty());
    assert_eq!(s.fake.messages("Deleted Messages").len(), 1);
}

#[tokio::test]
async fn smtp_failure_keeps_the_draft_and_files_nothing() {
    let s = setup().await;
    *s.sender.fail_with.lock().unwrap() = Some("550 5.1.1 mailbox does not exist".into());
    let r = s
        .server
        .send_draft(params(json!({"uid": s.draft_uid})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("sending failed"), "{}", text(&r));
    assert!(!text(&r).contains("WAS sent"));
    assert_eq!(s.fake.messages("Drafts").len(), 1);
    assert!(s.fake.messages("Sent Messages").is_empty());
}

#[tokio::test]
async fn sent_append_failure_reports_sent_and_keeps_the_draft() {
    let s = setup().await;
    s.fake
        .state
        .lock()
        .unwrap()
        .fail
        .insert("APPEND".into(), "over quota".into());
    let r = s
        .server
        .send_draft(params(json!({"uid": s.draft_uid})))
        .await
        .unwrap();
    assert!(is_error(&r));
    let msg = text(&r);
    assert!(msg.contains("WAS sent"), "{msg}");
    assert!(msg.contains("saving a copy to Sent"), "{msg}");
    assert!(msg.contains("over quota"), "{msg}");
    assert_eq!(s.sender.sent.lock().unwrap().len(), 1);
    assert_eq!(s.fake.messages("Drafts").len(), 1);
    assert!(s.fake.messages("Sent Messages").is_empty());
}

#[tokio::test]
async fn draft_removal_failure_reports_sent_and_keeps_the_draft() {
    let s = setup().await;
    s.fake
        .state
        .lock()
        .unwrap()
        .fail
        .insert("MOVE".into(), "simulated move failure".into());
    let r = s
        .server
        .send_draft(params(json!({"uid": s.draft_uid})))
        .await
        .unwrap();
    assert!(is_error(&r));
    let msg = text(&r);
    assert!(msg.contains("WAS sent"), "{msg}");
    assert!(msg.contains("removing the draft"), "{msg}");
    assert_eq!(s.fake.messages("Sent Messages").len(), 1);
    assert_eq!(s.fake.messages("Drafts").len(), 1);
}

#[tokio::test]
async fn draft_without_recipients_is_not_sent() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let sender = Arc::new(RecordingSender::default());
    let server = server(&fake, Some(sender.clone() as Arc<dyn Sender>));
    let v = json(
        &server
            .create_draft(params(json!({"subject": "note to self", "body_text": "x"})))
            .await
            .unwrap(),
    );
    let r = server
        .send_draft(params(json!({"uid": v["uid"]})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(sender.sent.lock().unwrap().is_empty());
    assert_eq!(fake.messages("Drafts").len(), 1);
}

#[tokio::test]
async fn stale_uidvalidity_sends_nothing() {
    let s = setup().await;
    let r = s
        .server
        .send_draft(params(json!({"uid": s.draft_uid, "uidvalidity": 5})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(s.sender.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_draft_is_never_sent_twice() {
    let s = setup().await;
    let first = s
        .server
        .send_draft(params(json!({"uid": s.draft_uid})))
        .await
        .unwrap();
    assert!(!is_error(&first), "{}", text(&first));
    let second = s
        .server
        .send_draft(params(json!({"uid": s.draft_uid})))
        .await
        .unwrap();
    assert!(is_error(&second));
    assert!(
        text(&second).contains("message not found"),
        "{}",
        text(&second)
    );
    assert_eq!(s.sender.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_sends_of_one_draft_send_once() {
    let s = setup().await;
    let other_session = s.server.clone();
    let (a, b) = tokio::join!(
        s.server.send_draft(params(json!({"uid": s.draft_uid}))),
        other_session.send_draft(params(json!({"uid": s.draft_uid}))),
    );
    let errors = [a.unwrap(), b.unwrap()]
        .iter()
        .filter(|r| is_error(r))
        .count();
    assert_eq!(errors, 1);
    assert_eq!(s.sender.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_message_without_the_draft_flag_is_refused() {
    let fake = FakeImap::start(FakeState::icloud_like()).await;
    let uid = fake.state.lock().unwrap().add_message(
        "Drafts",
        b"From: me@icloud.com\r\nTo: bob@example.com\r\nSubject: not a draft\r\n\r\nbody\r\n",
        &["\\Seen"],
    );
    let sender = Arc::new(RecordingSender::default());
    let server = server(&fake, Some(sender.clone() as Arc<dyn Sender>));
    let r = server
        .send_draft(params(json!({"uid": uid})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("message is not a draft"), "{}", text(&r));
    assert!(sender.sent.lock().unwrap().is_empty());
    assert_eq!(fake.messages("Drafts").len(), 1);
}
