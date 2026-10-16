mod support;
use serde_json::json;
use support::fake_imap::{FakeImap, FakeState};
use support::tools::{is_error, json, params, server, text};

const PLAIN: &[u8] = b"From: Alice <alice@example.com>\r\nTo: me@icloud.com, Bob <bob@example.com>\r\nSubject: Lunch\r\nMessage-ID: <t1@example.com>\r\nDate: Wed, 15 Oct 2026 10:00:00 +0000\r\n\r\nShall we meet at noon?\r\n";

const ATTACHMENT_ONLY: &[u8] = b"From: a@example.com\r\nTo: me@icloud.com\r\nSubject: scan\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: application/pdf; name=\"scan.pdf\"\r\nContent-Disposition: attachment; filename=\"scan.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b1--\r\n";

async fn fake() -> FakeImap {
    let mut st = FakeState::icloud_like();
    st.add_message("INBOX", PLAIN, &[]);
    st.add_message("INBOX", ATTACHMENT_ONLY, &[]);
    FakeImap::start(st).await
}

fn raw_text(fake: &FakeImap, mailbox: &str, idx: usize) -> String {
    String::from_utf8(fake.messages(mailbox)[idx].raw.clone()).unwrap()
}

#[tokio::test]
async fn organise_and_compose_tools_are_registered() {
    let fake = fake().await;
    let names = server(&fake, None).tool_names();
    for n in [
        "create_draft",
        "create_forward_draft",
        "create_mailbox",
        "create_reply_draft",
        "delete_messages",
        "mark_messages",
        "move_messages",
    ] {
        assert!(names.contains(&n.to_string()), "{names:?}");
    }
    assert_eq!(names.len(), 12);
}

#[tokio::test]
async fn mark_messages_sets_flags() {
    let fake = fake().await;
    let r = server(&fake, None)
        .mark_messages(params(
            json!({"uids": [1, 2], "seen": true, "flagged": true, "uidvalidity": 1000}),
        ))
        .await
        .unwrap();
    assert_eq!(json(&r)["updated"], true);
    for m in fake.messages("INBOX") {
        assert_eq!(m.flags, vec!["\\Seen", "\\Flagged"]);
    }
}

#[tokio::test]
async fn mark_messages_needs_a_flag() {
    let fake = fake().await;
    let r = server(&fake, None)
        .mark_messages(params(json!({"uids": [1]})))
        .await
        .unwrap();
    assert!(is_error(&r));
}

#[tokio::test]
async fn mark_messages_with_stale_uidvalidity_changes_nothing() {
    let fake = fake().await;
    let r = server(&fake, None)
        .mark_messages(params(json!({"uids": [1], "seen": true, "uidvalidity": 7})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("UIDVALIDITY"), "{}", text(&r));
    assert!(fake.messages("INBOX")[0].flags.is_empty());
}

#[tokio::test]
async fn tagged_no_on_store_is_a_tool_error() {
    let fake = fake().await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("STORE".into(), "simulated store failure".into());
    let r = server(&fake, None)
        .mark_messages(params(json!({"uids": [1], "seen": true})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("simulated store failure"), "{}", text(&r));
}

#[tokio::test]
async fn move_and_delete_messages() {
    let fake = fake().await;
    let s = server(&fake, None);
    json(
        &s.move_messages(params(json!({"uids": [1], "target_mailbox": "Archive"})))
            .await
            .unwrap(),
    );
    json(
        &s.delete_messages(params(json!({"uids": [2]})))
            .await
            .unwrap(),
    );
    assert!(fake.messages("INBOX").is_empty());
    assert_eq!(fake.messages("Archive").len(), 1);
    assert_eq!(fake.messages("Deleted Messages").len(), 1);
    let r = s
        .delete_messages(params(
            json!({"mailbox": "Archive", "uids": [1], "permanent": true}),
        ))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert_eq!(fake.messages("Archive").len(), 1);
}

#[tokio::test]
async fn create_mailbox_tool() {
    let fake = fake().await;
    json(
        &server(&fake, None)
            .create_mailbox(params(json!({"name": "Receipts"})))
            .await
            .unwrap(),
    );
    assert!(
        fake.state
            .lock()
            .unwrap()
            .mailboxes
            .contains_key("Receipts")
    );
}

#[tokio::test]
async fn create_draft_saves_to_drafts_and_keeps_bcc() {
    let fake = fake().await;
    let v = json(
        &server(&fake, None)
            .create_draft(params(json!({
                "to": ["Bob <bob@example.com>"],
                "bcc": ["secret@example.com"],
                "subject": "Plans",
                "body_text": "See you there."
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["mailbox"], "Drafts");
    assert_eq!(v["uid"], 1);
    let drafts = fake.messages("Drafts");
    assert_eq!(drafts[0].flags, vec!["\\Draft", "\\Seen"]);
    let raw = raw_text(&fake, "Drafts", 0);
    assert!(
        raw.contains("Bcc: <secret@example.com>") || raw.contains("Bcc: secret@example.com"),
        "{raw}"
    );
    assert!(raw.contains("Subject: Plans"));
    assert!(raw.contains(&format!("<{}>", v["message_id"].as_str().unwrap())));
}

#[tokio::test]
async fn create_reply_draft_is_threaded() {
    let fake = fake().await;
    json(
        &server(&fake, None)
            .create_reply_draft(params(
                json!({"uid": 1, "body_text": "Noon works.", "reply_all": true}),
            ))
            .await
            .unwrap(),
    );
    let raw = raw_text(&fake, "Drafts", 0);
    assert!(raw.contains("In-Reply-To: <t1@example.com>"), "{raw}");
    assert!(raw.contains("Subject: Re: Lunch"), "{raw}");
    assert!(raw.contains("alice@example.com"), "{raw}");
    assert!(raw.contains("bob@example.com"), "{raw}");
    assert!(raw.contains("> Shall we meet at noon?"), "{raw}");
}

#[tokio::test]
async fn create_forward_draft_reattaches_attachments() {
    let fake = fake().await;
    json(
        &server(&fake, None)
            .create_forward_draft(params(
                json!({"uid": 2, "to": ["carol@example.com"], "body_text": "FYI"}),
            ))
            .await
            .unwrap(),
    );
    let raw = raw_text(&fake, "Drafts", 0);
    assert!(raw.contains("Subject: Fwd: scan"), "{raw}");
    assert!(raw.contains("scan.pdf"), "{raw}");
}
