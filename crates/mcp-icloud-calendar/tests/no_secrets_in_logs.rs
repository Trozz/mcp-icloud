//! Runs discovery, a listing and a write with every log level enabled (and request/response
//! bodies logged) and checks that neither the app password nor the Basic auth header appears.
mod support;
use mcp_icloud_calendar::caldav::caldav_settings;
use mcp_icloud_calendar::caldav::discovery::Directory;
use mcp_icloud_calendar::tools::CalendarServer;
use mcp_icloud_dav::client::DavClient;
use secrecy::SecretString;
use serde_json::json;
use std::io::Write;
use std::sync::{Arc, Mutex};
use support::fake_caldav::{AUTH, EMAIL, FakeCalDav, FakeState, PASSWORD};
use support::tools::{json, params, settings};

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
async fn credentials_never_reach_the_logs() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);

    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let mut caldav = caldav_settings(fake.url.clone(), EMAIL.into(), SecretString::from(PASSWORD));
    caldav.log_bodies = true;
    let dir = Arc::new(Directory::new(
        Arc::new(DavClient::new(caldav).unwrap()),
        EMAIL.into(),
    ));
    let server = CalendarServer::new(dir, settings(false));
    json(&server.list_calendars().await.unwrap());
    json(
        &server
            .create_event(params(json!({
                "calendar": "Home", "summary": "Logged", "start_date": "2026-11-01"
            })))
            .await
            .unwrap(),
    );
    json(
        &server
            .list_events(params(json!({"start": "2026-11-01", "end": "2026-11-02"})))
            .await
            .unwrap(),
    );

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("caldav request"), "logging was not captured");
    assert!(
        logs.contains("SUMMARY:Logged"),
        "bodies are logged in this mode"
    );
    assert!(!logs.contains(PASSWORD), "the app password leaked");
    let encoded = AUTH.trim_start_matches("Basic ");
    assert!(!logs.contains(encoded), "the Basic auth header leaked");
}

#[tokio::test]
async fn unreadable_events_never_put_their_text_in_logs_or_results() {
    const SENTINEL: &str = "SENTINEL-NOT-A-CONTENT-LINE";
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);

    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    fake.state().put(
        "home",
        "broken.ics",
        &format!(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:broken-1\nDTSTART;VALUE=DATE:20261101\n{SENTINEL}\nEND:VEVENT\nEND:VCALENDAR\n"
        ),
    );
    let caldav = caldav_settings(fake.url.clone(), EMAIL.into(), SecretString::from(PASSWORD));
    let dir = Arc::new(Directory::new(
        Arc::new(DavClient::new(caldav).unwrap()),
        EMAIL.into(),
    ));
    let server = CalendarServer::new(dir, settings(false));
    let listed = server
        .list_events(params(json!({"start": "2026-11-01", "end": "2026-11-02"})))
        .await
        .unwrap();
    let got = server
        .get_event(params(
            json!({"event_id": FakeCalDav::event_id("home", "broken.ics")}),
        ))
        .await
        .unwrap();
    let texts = format!("{:?}{:?}", listed.content, got.content);
    assert!(got.is_error == Some(true), "{texts}");
    assert!(
        !texts.contains(SENTINEL),
        "event text reached a tool result: {texts}"
    );

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("skipping an unreadable event"),
        "the broken event was not listed"
    );
    assert!(!logs.contains(SENTINEL), "event text reached the logs");
}
