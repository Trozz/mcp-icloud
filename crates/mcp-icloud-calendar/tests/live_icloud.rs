//! Live tests against a real iCloud account. Ignored by default; they need:
//!   ICLOUD_EMAIL, ICLOUD_APP_PASSWORD
//!   ICLOUD_TEST_CALENDAR (optional, default "Home"): a writable calendar for the round trip
//!   ICLOUD_TEST_ATTENDEE (only for the invitation test): an address you control
//! Run: cargo test -p mcp-icloud-calendar --test live_icloud -- --ignored --nocapture --test-threads=1
//! The round trip creates one recurring event in December 2030 named "mcp-icloud-calendar live
//! test (safe to delete)" and always deletes it again, also when an assertion fails. Output
//! only prints counts, never event content.
mod support;
use futures::FutureExt;
use mcp_icloud_calendar::caldav::caldav_settings;
use mcp_icloud_calendar::caldav::discovery::Directory;
use mcp_icloud_calendar::config::DEFAULT_CALDAV_URL;
use mcp_icloud_calendar::tools::{CalendarServer, ToolSettings};
use mcp_icloud_calendar::tz::ZoneState;
use mcp_icloud_dav::client::DavClient;
use secrecy::SecretString;
use serde_json::{Value, json};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use support::tools::{error_text, json, params};
use url::Url;

const SUMMARY: &str = "mcp-icloud-calendar live test (safe to delete)";

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn directory() -> Arc<Directory> {
    let email = env("ICLOUD_EMAIL").expect("set ICLOUD_EMAIL to run the live tests");
    let password =
        env("ICLOUD_APP_PASSWORD").expect("set ICLOUD_APP_PASSWORD to run the live tests");
    let client = DavClient::new(caldav_settings(
        Url::parse(DEFAULT_CALDAV_URL).unwrap(),
        email.clone(),
        SecretString::from(password),
    ))
    .unwrap();
    Arc::new(Directory::new(Arc::new(client), email))
}

fn server(allow_invites: bool) -> (CalendarServer, Arc<Directory>) {
    let dir = directory();
    let settings = Arc::new(ToolSettings {
        zones: ZoneState::new(chrono_tz::Europe::London),
        allow_invites,
        email: env("ICLOUD_EMAIL").unwrap(),
    });
    (CalendarServer::new(dir.clone(), settings), dir)
}

fn calendar() -> String {
    env("ICLOUD_TEST_CALENDAR").unwrap_or_else(|| "Home".into())
}

/// Deletes every event named `SUMMARY` in December 2030 from the test calendar.
async fn cleanup(s: &CalendarServer) {
    let r = s
        .list_events(params(json!({
            "start": "2030-12-01", "end": "2031-01-01",
            "calendars": [calendar()], "query": SUMMARY
        })))
        .await
        .unwrap();
    let Ok(v) = serde_json::from_str::<Value>(&support::tools::text(&r)) else {
        return;
    };
    let mut ids: Vec<String> = v["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["event_id"].as_str().map(str::to_string))
        .collect();
    ids.dedup();
    for id in ids {
        let _ = s.delete_event(params(json!({"event_id": id}))).await;
    }
}

#[tokio::test]
#[ignore]
async fn live_discovery_and_listing() {
    let (s, _) = server(false);
    let cals = json(&s.list_calendars().await.unwrap());
    let all = cals["calendars"].as_array().unwrap();
    let writable = all.iter().filter(|c| c["writable"] == true).count();
    println!("calendars: {} ({} writable)", all.len(), writable);
    assert!(writable >= 1, "expected at least one writable calendar");
    let v = json(&s.list_events(params(json!({}))).await.unwrap());
    println!(
        "events in the next 7 days: {}",
        v["events"].as_array().unwrap().len()
    );
    let today = chrono::Utc::now().date_naive();
    let end = today + chrono::Duration::days(30);
    let v = json(
        &s.list_events(params(json!({
            "start": today.to_string(), "end": end.to_string()
        })))
        .await
        .unwrap(),
    );
    println!(
        "events in the next 30 days: {} (truncated: {})",
        v["events"].as_array().unwrap().len(),
        v["truncated"]
    );
}

#[tokio::test]
#[ignore]
async fn live_round_trip() {
    let (s, _) = server(false);
    cleanup(&s).await;
    let outcome = AssertUnwindSafe(async {
        let created = json(
            &s.create_event(params(json!({
                "calendar": calendar(),
                "summary": SUMMARY,
                "start": "2030-12-02T09:00",
                "end": "2030-12-02T09:30",
                "recurrence": {"freq": "weekly", "count": 3},
                "alerts_minutes_before": [10]
            })))
            .await
            .unwrap(),
        );
        let id = created["event_id"].as_str().unwrap().to_string();
        println!("created {id}");
        let got = json(&s.get_event(params(json!({"event_id": id}))).await.unwrap());
        assert_eq!(got["summary"], SUMMARY);
        assert_eq!(got["start"], "2030-12-02T09:00:00+00:00");
        assert_eq!(got["alerts_minutes_before"], json!([10]));

        json(
            &s.update_event(params(json!({
                "event_id": id, "scope": "occurrence",
                "occurrence_start": "2030-12-09T09:00:00+00:00",
                "start": "2030-12-09T11:00"
            })))
            .await
            .unwrap(),
        );
        let moved = json(
            &s.get_event(params(json!({
                "event_id": id, "occurrence_start": "2030-12-09T09:00:00+00:00"
            })))
            .await
            .unwrap(),
        );
        assert_eq!(moved["start"], "2030-12-09T11:00:00+00:00");

        json(
            &s.delete_event(params(json!({
                "event_id": id, "scope": "occurrence",
                "occurrence_start": "2030-12-16T09:00:00+00:00"
            })))
            .await
            .unwrap(),
        );
        let listed = json(
            &s.list_events(params(json!({
                "start": "2030-12-01", "end": "2031-01-01",
                "calendars": [calendar()], "query": SUMMARY
            })))
            .await
            .unwrap(),
        );
        let starts: Vec<&str> = listed["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["start"].as_str().unwrap())
            .collect();
        assert_eq!(
            starts,
            ["2030-12-02T09:00:00+00:00", "2030-12-09T11:00:00+00:00"]
        );

        json(
            &s.delete_event(params(json!({"event_id": id})))
                .await
                .unwrap(),
        );
        let gone = error_text(&s.get_event(params(json!({"event_id": id}))).await.unwrap());
        assert!(gone.starts_with("not_found:"), "{gone}");
    })
    .catch_unwind()
    .await;
    cleanup(&s).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Checks that iCloud schedules invitations itself when an event with attendees is PUT
/// (RFC 6638): the attendee line gets a SCHEDULE-STATUS. Sends one real invitation and one
/// cancellation to ICLOUD_TEST_ATTENDEE.
#[tokio::test]
#[ignore]
async fn live_invitation_scheduling() {
    let Some(attendee) = env("ICLOUD_TEST_ATTENDEE") else {
        println!("ICLOUD_TEST_ATTENDEE is not set; skipping");
        return;
    };
    let (s, dir) = server(true);
    cleanup(&s).await;
    let outcome = AssertUnwindSafe(async {
        let created = json(
            &s.create_event(params(json!({
                "calendar": calendar(),
                "summary": SUMMARY,
                "start": "2030-12-03T10:00",
                "end": "2030-12-03T10:30",
                "attendees": [{"email": attendee}]
            })))
            .await
            .unwrap(),
        );
        let id = created["event_id"].as_str().unwrap().to_string();
        let account = dir.account().await.unwrap();
        let (url, _) = account.resolve_event(&id).unwrap();
        let raw = dir
            .client()
            .get(&url)
            .await
            .unwrap()
            .body
            .replace("\r\n ", "");
        let line = raw
            .lines()
            .find(|l| {
                l.starts_with("ATTENDEE") && l.to_lowercase().contains(&attendee.to_lowercase())
            })
            .expect("the attendee line is stored")
            .to_string();
        println!(
            "attendee line parameters: {}",
            line.split(':').next().unwrap()
        );
        assert!(
            line.contains("SCHEDULE-STATUS="),
            "iCloud did not report scheduling for the attendee; keep ICLOUD_ALLOW_INVITES=false"
        );
        println!("check {attendee}'s inbox for the invitation, then for the cancellation");
        json(
            &s.delete_event(params(json!({"event_id": id})))
                .await
                .unwrap(),
        );
    })
    .catch_unwind()
    .await;
    cleanup(&s).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
