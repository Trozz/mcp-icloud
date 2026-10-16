mod support;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rmcp::model::ResourceContents;
use serde_json::json;
use support::fake_imap::{FakeImap, FakeState};
use support::tools::{is_error, json, params, server, settings, text};

const PLAIN: &[u8] = b"From: Alice <alice@example.com>\r\nTo: me@icloud.com\r\nSubject: Lunch\r\nMessage-ID: <t1@example.com>\r\nDate: Wed, 15 Oct 2026 10:00:00 +0000\r\n\r\nShall we meet at noon?\r\n";

const REPLY: &[u8] = b"From: Me <me@icloud.com>\r\nTo: alice@example.com\r\nSubject: Re: Lunch\r\nMessage-ID: <t2@icloud.com>\r\nIn-Reply-To: <t1@example.com>\r\nReferences: <t1@example.com>\r\nDate: Wed, 15 Oct 2026 11:00:00 +0000\r\n\r\nYes.\r\n";

const ATTACHMENT_ONLY: &[u8] = b"From: a@example.com\r\nTo: me@icloud.com\r\nSubject: scan\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: application/pdf; name=\"scan.pdf\"\r\nContent-Disposition: attachment; filename=\"scan.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b1--\r\n";

async fn fake() -> FakeImap {
    let mut st = FakeState::icloud_like();
    st.add_message("INBOX", PLAIN, &[]);
    st.add_message("INBOX", ATTACHMENT_ONLY, &["\\Seen"]);
    st.add_message("Sent Messages", REPLY, &["\\Seen"]);
    FakeImap::start(st).await
}

#[tokio::test]
async fn read_tools_are_registered_and_send_is_absent() {
    let fake = fake().await;
    let names = server(&fake, None).tool_names();
    for n in [
        "get_attachment",
        "get_message",
        "get_thread",
        "list_mailboxes",
        "search_messages",
    ] {
        assert!(names.contains(&n.to_string()), "{names:?}");
    }
    assert!(!names.contains(&"send_draft".to_string()));
}

#[tokio::test]
async fn list_mailboxes_reports_roles_and_counts() {
    let fake = fake().await;
    let v = json(&server(&fake, None).list_mailboxes().await.unwrap());
    let inbox = v["mailboxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "INBOX")
        .unwrap();
    assert_eq!(inbox["role"], "inbox");
    assert_eq!(inbox["total"], 2);
    assert_eq!(inbox["unseen"], 1);
}

#[tokio::test]
async fn search_messages_returns_a_page() {
    let fake = fake().await;
    let r = server(&fake, None)
        .search_messages(params(json!({})))
        .await
        .unwrap();
    let v = json(&r);
    assert_eq!(v["mailbox"], "INBOX");
    assert_eq!(v["uidvalidity"], 1000);
    let uids: Vec<u64> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["uid"].as_u64().unwrap())
        .collect();
    assert_eq!(uids, vec![2, 1]);
}

#[tokio::test]
async fn bad_search_date_is_a_tool_error() {
    let fake = fake().await;
    let r = server(&fake, None)
        .search_messages(params(json!({"since": "15/10/2026"})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("YYYY-MM-DD"), "{}", text(&r));
}

#[tokio::test]
async fn rejected_search_is_a_tool_error_not_an_empty_page() {
    let fake = fake().await;
    fake.state
        .lock()
        .unwrap()
        .fail
        .insert("SEARCH".into(), "simulated search failure".into());
    let r = server(&fake, None)
        .search_messages(params(json!({})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(
        text(&r).contains("simulated search failure"),
        "{}",
        text(&r)
    );
}

#[tokio::test]
async fn get_message_returns_text_body() {
    let fake = fake().await;
    let v = json(
        &server(&fake, None)
            .get_message(params(json!({"uid": 1})))
            .await
            .unwrap(),
    );
    assert_eq!(v["uidvalidity"], 1000);
    assert_eq!(v["message"]["subject"], "Lunch");
    assert!(
        v["message"]["body"]
            .as_str()
            .unwrap()
            .contains("Shall we meet at noon?")
    );
}

#[tokio::test]
async fn attachment_only_message_has_empty_body_and_attachment_list() {
    let fake = fake().await;
    let v = json(
        &server(&fake, None)
            .get_message(params(json!({"mailbox": "INBOX", "uid": 2})))
            .await
            .unwrap(),
    );
    assert_eq!(v["message"]["body"], "");
    assert_eq!(v["message"]["attachments"][0]["filename"], "scan.pdf");
    assert_eq!(
        v["message"]["attachments"][0]["mime_type"],
        "application/pdf"
    );
}

#[tokio::test]
async fn missing_message_is_a_tool_error() {
    let fake = fake().await;
    let r = server(&fake, None)
        .get_message(params(json!({"uid": 99})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("message not found"), "{}", text(&r));
}

#[tokio::test]
async fn get_attachment_returns_a_blob_resource() {
    let fake = fake().await;
    let r = server(&fake, None)
        .get_attachment(params(json!({"uid": 2, "index": 0})))
        .await
        .unwrap();
    assert!(!is_error(&r), "{}", text(&r));
    let meta: serde_json::Value = serde_json::from_str(&text(&r)).unwrap();
    assert_eq!(meta["filename"], "scan.pdf");
    let res = &r.content[1]
        .as_resource()
        .expect("embedded resource")
        .resource;
    let ResourceContents::BlobResourceContents {
        mime_type,
        blob,
        uri,
        ..
    } = res
    else {
        panic!("expected a blob resource")
    };
    assert_eq!(mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(STANDARD.decode(blob).unwrap(), b"%PDF-1.4\n");
    assert_eq!(uri, "icloud-mail://INBOX/2/attachments/0");
}

#[tokio::test]
async fn oversized_attachment_is_refused() {
    let fake = fake().await;
    let s = mcp_icloud_mail::tools::MailServer::new(support::tools::imap(&fake), None, settings(4));
    let r = s
        .get_attachment(params(json!({"uid": 2, "index": 0})))
        .await
        .unwrap();
    assert!(is_error(&r));
    assert!(text(&r).contains("MAX_ATTACHMENT_BYTES"), "{}", text(&r));
}

#[tokio::test]
async fn get_thread_includes_the_reply_in_sent() {
    let fake = fake().await;
    fake.state
        .lock()
        .unwrap()
        .mailboxes
        .get_mut("Sent Messages")
        .unwrap()
        .uidvalidity = 2000;
    let v = json(
        &server(&fake, None)
            .get_thread(params(json!({"uid": 1})))
            .await
            .unwrap(),
    );
    let entries: Vec<(String, String, u64)> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["mailbox"].as_str().unwrap().to_string(),
                m["subject"].as_str().unwrap().to_string(),
                m["uidvalidity"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        vec![
            ("INBOX".to_string(), "Lunch".to_string(), 1000),
            ("Sent Messages".to_string(), "Re: Lunch".to_string(), 2000),
        ]
    );
}
