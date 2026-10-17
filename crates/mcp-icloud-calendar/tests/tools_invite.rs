//! Invitations with ICLOUD_ALLOW_INVITES=true.
mod support;
use serde_json::json;
use support::events::*;
use support::fake_caldav::{FakeCalDav, FakeState};
use support::tools::{error_text, json, params, server};

const RECURRING_INVITE: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Example Corp//Calendar//EN
BEGIN:VEVENT
UID:weekly-invite-1
DTSTAMP:20261001T080000Z
DTSTART:20261102T150000Z
DTEND:20261102T153000Z
RRULE:FREQ=WEEKLY;COUNT=4
SUMMARY:Weekly sync
ORGANIZER:mailto:olive@example.net
ATTENDEE;PARTSTAT=ACCEPTED:mailto:olive@example.net
ATTENDEE;PARTSTAT=ACCEPTED:mailto:user@example.com
SEQUENCE:1
END:VEVENT
END:VCALENDAR
";

async fn fake() -> FakeCalDav {
    let mut st = FakeState::icloud_like();
    st.put("home", "dentist-1.ics", DENTIST);
    st.put("work", "invite-1.ics", INVITE);
    st.put("work", "mine-1.ics", MY_MEETING);
    st.put("work", "weekly-invite-1.ics", RECURRING_INVITE);
    FakeCalDav::start(st).await
}

fn unfolded(fake: &FakeCalDav, cal: &str, name: &str) -> String {
    fake.state()
        .resource(cal, name)
        .expect("resource exists")
        .body
        .replace("\r\n ", "")
}

#[tokio::test]
async fn respond_to_invitation_is_only_registered_when_enabled() {
    let fake = fake().await;
    assert!(
        server(&fake, true)
            .tool_names()
            .contains(&"respond_to_invitation".to_string())
    );
    assert!(
        !server(&fake, false)
            .tool_names()
            .contains(&"respond_to_invitation".to_string())
    );
}

#[tokio::test]
async fn accepting_sets_my_partstat_only() {
    let fake = fake().await;
    let s = server(&fake, true);
    let before = unfolded(&fake, "work", "invite-1.ics");
    let v = json(
        &s.respond_to_invitation(params(json!({
            "event_id": "/1234567890/calendars/work/invite-1.ics",
            "response": "accepted"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["my_status"], "accepted");
    let after = unfolded(&fake, "work", "invite-1.ics");
    assert!(
        after.contains("ATTENDEE;CN=Example User;PARTSTAT=ACCEPTED;EMAIL=user.alias@example.org:urn:uuid:00000000-0000-4000-8000-000000000001\r\n"),
        "{after}"
    );
    assert!(
        after.contains("SEQUENCE:2\r\n"),
        "an attendee reply keeps SEQUENCE"
    );
    for line in before
        .lines()
        .filter(|l| l.starts_with("ORGANIZER") || l.contains("olive@"))
    {
        assert!(after.contains(line), "organizer data changed: {line}");
    }
    let v = json(
        &s.list_events(params(
            json!({"start": "2026-10-19", "end": "2026-10-27", "needs_response": true}),
        ))
        .await
        .unwrap(),
    );
    assert!(v["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn declining_one_occurrence_adds_an_override() {
    let fake = fake().await;
    let v = json(
        &server(&fake, true)
            .respond_to_invitation(params(json!({
                "event_id": "/1234567890/calendars/work/weekly-invite-1.ics",
                "response": "declined",
                "occurrence_start": "2026-11-09T15:00:00Z"
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["occurrence_start"], "2026-11-09T15:00:00+00:00");
    let ics = unfolded(&fake, "work", "weekly-invite-1.ics");
    assert!(ics.contains("RECURRENCE-ID:20261109T150000Z\r\n"), "{ics}");
    assert!(ics.contains("PARTSTAT=DECLINED"), "{ics}");
    assert_eq!(
        ics.matches("ATTENDEE;PARTSTAT=ACCEPTED:mailto:user@example.com")
            .count(),
        1,
        "the master keeps my acceptance"
    );
}

#[tokio::test]
async fn replies_are_refused_for_non_invitations_and_bad_answers() {
    let fake = fake().await;
    let s = server(&fake, true);
    for (id, response) in [
        ("/1234567890/calendars/work/mine-1.ics", "accepted"),
        ("/1234567890/calendars/home/dentist-1.ics", "accepted"),
        ("/1234567890/calendars/work/invite-1.ics", "maybe"),
    ] {
        let e = error_text(
            &s.respond_to_invitation(params(json!({"event_id": id, "response": response})))
                .await
                .unwrap(),
        );
        assert!(e.starts_with("validation:"), "{id}: {e}");
    }
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn creating_with_attendees_makes_me_the_organizer() {
    let fake = fake().await;
    let v = json(
        &server(&fake, true)
            .create_event(params(json!({
                "calendar": "Work",
                "summary": "Kick-off",
                "start": "2026-11-05T10:00",
                "end": "2026-11-05T11:00",
                "attendees": [{"email": "bob@example.com", "name": "Bob"}]
            })))
            .await
            .unwrap(),
    );
    let name = v["event_id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let ics = unfolded(&fake, "work", &name);
    assert!(ics.contains("ORGANIZER"), "{ics}");
    assert!(ics.contains(":mailto:user@example.com\r\n"), "{ics}");
    assert!(ics.contains("mailto:bob@example.com\r\n"), "{ics}");
    assert!(ics.contains("PARTSTAT=NEEDS-ACTION"), "{ics}");
}

#[tokio::test]
async fn my_meetings_can_be_changed_and_attendees_edited_when_enabled() {
    let fake = fake().await;
    let s = server(&fake, true);
    let id = "/1234567890/calendars/work/mine-1.ics";
    let bob = unfolded(&fake, "work", "mine-1.ics")
        .lines()
        .find(|l| l.contains("bob@example.com"))
        .unwrap()
        .to_string();
    json(
        &s.update_event(params(json!({
            "event_id": id,
            "summary": "Planning (moved)",
            "attendees": [{"email": "bob@example.com"}, {"email": "carol@example.org"}]
        })))
        .await
        .unwrap(),
    );
    let ics = unfolded(&fake, "work", "mine-1.ics");
    assert!(ics.contains("SUMMARY:Planning (moved)\r\n"));
    assert!(
        ics.contains("SEQUENCE:1\r\n"),
        "the organizer bumps SEQUENCE"
    );
    assert!(
        ics.contains(&bob),
        "existing attendees keep their line: {ics}"
    );
    assert!(ics.contains("mailto:carol@example.org\r\n"), "{ics}");
    json(
        &s.update_event(params(json!({"event_id": id, "attendees": []})))
            .await
            .unwrap(),
    );
    let ics = unfolded(&fake, "work", "mine-1.ics");
    assert!(
        !ics.contains("ATTENDEE") && !ics.contains("ORGANIZER"),
        "{ics}"
    );
}

#[tokio::test]
async fn received_invitations_still_only_accept_alerts_and_can_be_deleted() {
    let fake = fake().await;
    let s = server(&fake, true);
    let id = "/1234567890/calendars/work/invite-1.ics";
    let e = error_text(
        &s.update_event(params(json!({"event_id": id, "summary": "x"})))
            .await
            .unwrap(),
    );
    assert!(e.contains("respond_to_invitation"), "{e}");
    json(
        &s.delete_event(params(json!({"event_id": id})))
            .await
            .unwrap(),
    );
    assert!(fake.state().resource("work", "invite-1.ics").is_none());
}
