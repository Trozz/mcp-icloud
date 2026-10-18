//! Runs a full load, a search, a read, an update, a contact and a group export, a refused create
//! and a read of a card that cannot be parsed with every log level enabled and content logging
//! off, and checks that neither credentials nor contact content reach the logs.
mod support;
use serde_json::json;
use std::io::Write;
use std::sync::{Arc, Mutex};
use support::cards::{GROUP_UID, PERSON_UID, seed};
use support::fake_carddav::{AUTH, FakeCardDav, FakeState, PASSWORD};
use support::tools::{error_text, json, params, server};

const BROKEN_UID: &str = "BROKEN-1";
/// A line without a colon: the shared line layer rejects the card and its parse error quotes the
/// line, so this token must reach neither a log line nor a tool result.
const SENTINEL: &str = "SENTINEL-NOT-A-CONTENT-LINE";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn credentials_and_contact_content_never_reach_the_logs() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);

    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state.put(
        "BROKEN.vcf",
        &format!("BEGIN:VCARD\nVERSION:3.0\nUID:{BROKEN_UID}\nFN:Broken\n{SENTINEL}\nEND:VCARD\n"),
    );
    let fake = FakeCardDav::start(state).await;
    let s = server(&fake);
    json(
        &s.search_contacts(params(json!({"query": "alex"})))
            .await
            .unwrap(),
    );
    json(
        &s.get_contact(params(json!({"id": PERSON_UID})))
            .await
            .unwrap(),
    );
    json(
        &s.update_contact(params(json!({"id": PERSON_UID, "nickname": "Al"})))
            .await
            .unwrap(),
    );
    for id in [PERSON_UID, GROUP_UID] {
        json(&s.export_vcard(params(json!({"id": id}))).await.unwrap());
    }
    error_text(
        &s.create_contact(params(json!({
            "given_name": "Zebedee",
            "emails": [{"value": "not-an-address"}]
        })))
        .await
        .unwrap(),
    );
    let e = error_text(
        &s.get_contact(params(json!({"id": BROKEN_UID})))
            .await
            .unwrap(),
    );
    assert_eq!(
        e,
        format!(
            "upstream: iCloud error: the card {BROKEN_UID} could not be parsed; it is left unchanged"
        )
    );

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("carddav request"), "logging was not captured");
    assert!(logs.contains("tool call"));
    assert!(
        logs.contains("a card could not be parsed"),
        "the unparseable card was not loaded"
    );
    assert!(!logs.contains(PASSWORD), "the app password leaked");
    let encoded = AUTH.trim_start_matches("Basic ");
    assert!(!logs.contains(encoded), "the Basic auth header leaked");
    // A bare "7700" also matches the microseconds of a log timestamp, so the phone number is
    // matched with the digits that follow it.
    for content in [
        "Alex",
        "alex@example.com",
        "7700 900",
        "7700900",
        "Exampleton",
        "Chief Tinkerer",
        "Zebedee",
        "not-an-address",
        SENTINEL,
    ] {
        assert!(!logs.contains(content), "contact content leaked: {content}");
    }
}
