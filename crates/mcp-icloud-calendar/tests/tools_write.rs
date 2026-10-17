mod support;
use serde_json::{Value, json};
use support::events::*;
use support::fake_caldav::{FakeCalDav, FakeState};
use support::tools::{error_text, json, params, server};

async fn fake() -> FakeCalDav {
    let mut st = FakeState::icloud_like();
    st.put("home", "dentist-1.ics", DENTIST);
    st.put("work", "standup-1.ics", STANDUP);
    st.put("work", "invite-1.ics", INVITE);
    st.put("work", "mine-1.ics", MY_MEETING);
    FakeCalDav::start(st).await
}

fn body(fake: &FakeCalDav, cal: &str, name: &str) -> String {
    fake.state()
        .resource(cal, name)
        .expect("resource exists")
        .body
}

fn name_of(event_id: &Value) -> String {
    event_id
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

async fn summaries(fake: &FakeCalDav, start: &str, end: &str) -> Vec<String> {
    let v = json(
        &server(fake, false)
            .list_events(params(json!({"start": start, "end": end})))
            .await
            .unwrap(),
    );
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{} {}",
                e["start"].as_str().unwrap_or_default(),
                e["summary"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn write_tools_are_registered_without_invitation_replies() {
    let fake = fake().await;
    assert_eq!(
        server(&fake, false).tool_names(),
        [
            "create_event",
            "delete_event",
            "find_free_time",
            "get_event",
            "get_timezone",
            "list_calendars",
            "list_events",
            "set_timezone",
            "update_event"
        ]
    );
}

#[tokio::test]
async fn create_writes_a_new_resource_with_a_tzid_and_no_vtimezone() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .create_event(params(json!({
                "calendar": "Home",
                "summary": "Haircut, short",
                "start": "2026-11-03T14:00",
                "end": "2026-11-03T14:45",
                "location": "Barber",
                "alerts_minutes_before": [15]
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["created"], true);
    assert_eq!(v["calendar"], "Home");
    assert_eq!(v["start"], "2026-11-03T14:00:00+00:00");
    let name = name_of(&v["event_id"]);
    assert!(name.ends_with(".ics"));
    let ics = body(&fake, "home", &name);
    assert!(ics.contains("PRODID:-//Trozz//mcp-icloud//EN\r\n"), "{ics}");
    assert!(
        ics.contains("DTSTART;TZID=Europe/London:20261103T140000\r\n"),
        "{ics}"
    );
    assert!(
        ics.contains("DTEND;TZID=Europe/London:20261103T144500\r\n"),
        "{ics}"
    );
    assert!(ics.contains("SUMMARY:Haircut\\, short\r\n"), "{ics}");
    assert!(ics.contains("TRIGGER:-PT15M"), "{ics}");
    assert!(!ics.contains("VTIMEZONE"));
    let s = fake.state();
    let put = s.log.iter().find(|l| l.method == "PUT").unwrap();
    assert_eq!(put.if_none_match.as_deref(), Some("*"));
    assert_eq!(put.if_match, None);
}

#[tokio::test]
async fn create_all_day_stores_an_exclusive_end_and_reads_back_inclusive() {
    let fake = fake().await;
    let s = server(&fake, false);
    let v = json(
        &s.create_event(params(json!({
            "calendar": "Home",
            "summary": "Christmas",
            "start_date": "2026-12-24",
            "end_date": "2026-12-26"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["all_day"], true);
    assert_eq!(v["end_date"], "2026-12-26");
    let ics = body(&fake, "home", &name_of(&v["event_id"]));
    assert!(ics.contains("DTSTART;VALUE=DATE:20261224\r\n"), "{ics}");
    assert!(ics.contains("DTEND;VALUE=DATE:20261227\r\n"), "{ics}");
    let got = json(
        &s.get_event(params(json!({"event_id": v["event_id"]})))
            .await
            .unwrap(),
    );
    assert_eq!(got["start_date"], "2026-12-24");
    assert_eq!(got["end_date"], "2026-12-26");
}

#[tokio::test]
async fn create_refuses_a_time_in_the_dst_gap_without_writing() {
    let fake = fake().await;
    let e = error_text(
        &server(&fake, false)
            .create_event(params(json!({
                "calendar": "Home",
                "summary": "Too early",
                "start": "2026-03-29T01:30:00",
                "end": "2026-03-29T03:00:00"
            })))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
    assert!(e.contains("daylight saving gap"), "{e}");
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn create_with_a_recurrence_and_another_zone() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .create_event(params(json!({
                "calendar": "work",
                "summary": "Gym",
                "start": "2026-11-02T07:00",
                "end": "2026-11-02T08:00",
                "timezone": "America/New_York",
                "recurrence": {"freq": "weekly", "by_day": ["MO", "WE"], "count": 4}
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["start"], "2026-11-02T07:00:00-05:00");
    let ics = body(&fake, "work", &name_of(&v["event_id"]));
    assert!(
        ics.contains("DTSTART;TZID=America/New_York:20261102T070000\r\n"),
        "{ics}"
    );
    assert!(ics.contains("RRULE:FREQ=WEEKLY;"), "{ics}");
    let gyms: Vec<String> = summaries(&fake, "2026-11-01", "2026-11-30")
        .await
        .into_iter()
        .filter(|s| s.ends_with("Gym"))
        .collect();
    assert_eq!(
        gyms,
        [
            "2026-11-02T12:00:00+00:00 Gym",
            "2026-11-04T12:00:00+00:00 Gym",
            "2026-11-09T12:00:00+00:00 Gym",
            "2026-11-11T12:00:00+00:00 Gym"
        ]
    );
}

#[tokio::test]
async fn create_is_refused_on_read_only_calendars_and_with_attendees_when_invites_are_off() {
    let fake = fake().await;
    let s = server(&fake, false);
    let e = error_text(
        &s.create_event(params(json!({
            "calendar": "Holidays", "summary": "x", "start_date": "2026-11-01"
        })))
        .await
        .unwrap(),
    );
    assert!(e.starts_with("read_only:"), "{e}");
    let e = error_text(
        &s.create_event(params(json!({
            "calendar": "Home", "summary": "x", "start_date": "2026-11-01",
            "attendees": [{"email": "bob@example.com"}]
        })))
        .await
        .unwrap(),
    );
    assert!(e.contains("ICLOUD_ALLOW_INVITES=false"), "{e}");
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn update_series_changes_only_the_given_fields() {
    let fake = fake().await;
    let etag = fake.state().resource("home", "dentist-1.ics").unwrap().etag;
    let v = json(
        &server(&fake, false)
            .update_event(params(json!({
                "event_id": "/1234567890/calendars/home/dentist-1.ics",
                "summary": "Dentist (check-up)",
                "start": "2026-10-20T11:00"
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["updated"], true);
    assert_eq!(v["scope"], "series");
    assert_eq!(v["start"], "2026-10-20T11:00:00+01:00");
    assert_eq!(
        v["end"], "2026-10-20T12:00:00+01:00",
        "the duration is kept"
    );
    let ics = body(&fake, "home", "dentist-1.ics");
    assert!(ics.contains("SUMMARY:Dentist (check-up)\r\n"), "{ics}");
    assert!(
        ics.contains("X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n"),
        "{ics}"
    );
    assert!(ics.contains("TRIGGER:-PT30M\r\n"), "{ics}");
    assert!(ics.contains("LOCATION:High Street Clinic\r\n"), "{ics}");
    let s = fake.state();
    let put = s.log.iter().find(|l| l.method == "PUT").unwrap();
    assert_eq!(put.if_match.as_deref(), Some(etag.as_str()));
}

#[tokio::test]
async fn update_conflicts_are_reported_once_without_retry() {
    let fake = fake().await;
    fake.state().fail.push_back(("PUT", 412));
    let e = error_text(
        &server(&fake, false)
            .update_event(params(json!({
                "event_id": "/1234567890/calendars/home/dentist-1.ics",
                "summary": "Changed"
            })))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("conflict:"), "{e}");
    assert!(e.contains("re-read it and retry"), "{e}");
    assert_eq!(fake.state().count("PUT"), 1);
    assert!(body(&fake, "home", "dentist-1.ics").contains("SUMMARY:Dentist\r\n"));
}

#[tokio::test]
async fn update_one_occurrence_adds_an_override() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .update_event(params(json!({
                "event_id": "/1234567890/calendars/work/standup-1.ics",
                "scope": "occurrence",
                "occurrence_start": "2026-11-02T09:00:00+00:00",
                "summary": "Standup (demo)",
                "start": "2026-11-02T11:00"
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["occurrence_start"], "2026-11-02T09:00:00+00:00");
    assert_eq!(v["start"], "2026-11-02T11:00:00+00:00");
    let ics = body(&fake, "work", "standup-1.ics");
    assert!(
        ics.contains("RECURRENCE-ID;TZID=Europe/London:20261102T090000\r\n"),
        "{ics}"
    );
    assert!(
        ics.contains("SUMMARY:Standup\r\n"),
        "the master keeps its summary"
    );
    assert_eq!(
        summaries(&fake, "2026-11-02", "2026-11-10").await,
        [
            "2026-11-02T11:00:00+00:00 Standup (demo)",
            "2026-11-09T09:00:00+00:00 Standup"
        ]
    );
}

#[tokio::test]
async fn update_this_and_following_splits_the_series() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .update_event(params(json!({
                "event_id": "/1234567890/calendars/work/standup-1.ics",
                "scope": "this_and_following",
                "occurrence_start": "2026-11-09T09:00:00+00:00",
                "summary": "Standup v2"
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["old_series_deleted"], false);
    let new_name = name_of(&v["new_event_id"]);
    assert_ne!(new_name, "standup-1.ics");
    assert!(body(&fake, "work", "standup-1.ics").contains("UNTIL="));
    assert!(body(&fake, "work", &new_name).contains("SUMMARY:Standup v2\r\n"));
    assert_eq!(
        summaries(&fake, "2026-11-02", "2026-11-17").await,
        [
            "2026-11-02T09:00:00+00:00 Standup",
            "2026-11-09T09:00:00+00:00 Standup v2",
            "2026-11-16T09:00:00+00:00 Standup v2"
        ]
    );
}

#[tokio::test]
async fn moving_a_series_start_is_refused_when_occurrences_were_removed_or_changed() {
    let fake = fake().await;
    let s = server(&fake, false);
    let id = "/1234567890/calendars/work/standup-1.ics";
    let before = body(&fake, "work", "standup-1.ics");
    let e = error_text(
        &s.update_event(params(json!({"event_id": id, "start": "2026-10-06T09:00"})))
            .await
            .unwrap(),
    );
    assert!(e.contains("start cannot be moved"), "{e}");
    let e = error_text(
        &s.update_event(params(json!({
            "event_id": id,
            "scope": "this_and_following",
            "occurrence_start": "2026-10-12T09:00:00+01:00",
            "start": "2026-10-13T09:00"
        })))
        .await
        .unwrap(),
    );
    assert!(e.contains("start cannot be moved"), "{e}");
    {
        let st = fake.state();
        assert_eq!(st.count("PUT") + st.count("DELETE"), 0);
    }
    assert_eq!(body(&fake, "work", "standup-1.ics"), before);
    json(
        &s.update_event(params(json!({
            "event_id": id,
            "start": "2026-10-05T09:00",
            "end": "2026-10-05T09:30"
        })))
        .await
        .unwrap(),
    );
}

#[tokio::test]
async fn moving_the_start_of_the_following_occurrences_works_without_exceptions() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .update_event(params(json!({
                "event_id": "/1234567890/calendars/work/standup-1.ics",
                "scope": "this_and_following",
                "occurrence_start": "2026-11-02T09:00:00+00:00",
                "start": "2026-11-02T10:00"
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["start"], "2026-11-02T10:00:00+00:00");
    assert_eq!(
        summaries(&fake, "2026-10-26", "2026-11-10").await,
        [
            "2026-10-26T10:00:00+00:00 Standup (late)",
            "2026-11-02T10:00:00+00:00 Standup",
            "2026-11-09T10:00:00+00:00 Standup"
        ]
    );
}

#[tokio::test]
async fn received_invitations_are_never_split() {
    let fake = fake().await;
    let e = error_text(
        &server(&fake, true)
            .update_event(params(json!({
                "event_id": "/1234567890/calendars/work/invite-1.ics",
                "scope": "this_and_following",
                "occurrence_start": "2026-10-21T13:00:00+00:00",
                "alerts_minutes_before": [10]
            })))
            .await
            .unwrap(),
    );
    assert!(e.contains("invitation from olive@example.net"), "{e}");
    assert!(e.contains("this_and_following"), "{e}");
    let st = fake.state();
    assert_eq!(st.count("PUT") + st.count("DELETE"), 0);
    assert_eq!(st.resource_names("work").len(), 3);
}

#[tokio::test]
async fn received_invitations_only_accept_alert_changes() {
    let fake = fake().await;
    let s = server(&fake, false);
    let id = "/1234567890/calendars/work/invite-1.ics";
    let e = error_text(
        &s.update_event(params(json!({"event_id": id, "summary": "Mine now"})))
            .await
            .unwrap(),
    );
    assert!(e.contains("invitation from olive@example.net"), "{e}");
    let before = body(&fake, "work", "invite-1.ics");
    json(
        &s.update_event(params(
            json!({"event_id": id, "alerts_minutes_before": [10]}),
        ))
        .await
        .unwrap(),
    );
    // Long lines are re-folded at 75 octets on write; compare unfolded content.
    let after = body(&fake, "work", "invite-1.ics").replace("\r\n ", "");
    assert!(after.contains("TRIGGER:-PT10M"), "{after}");
    assert!(
        after.contains("SEQUENCE:2\r\n"),
        "an attendee must not bump SEQUENCE"
    );
    for line in before
        .lines()
        .filter(|l| l.starts_with("ATTENDEE") || l.starts_with("ORGANIZER"))
    {
        assert!(after.contains(line), "attendee line changed: {line}");
    }
}

#[tokio::test]
async fn my_meetings_with_attendees_are_not_changed_when_invites_are_off() {
    let fake = fake().await;
    let s = server(&fake, false);
    let id = "/1234567890/calendars/work/mine-1.ics";
    let e = error_text(
        &s.update_event(params(json!({"event_id": id, "summary": "Moved"})))
            .await
            .unwrap(),
    );
    assert!(e.contains("would email them"), "{e}");
    let e = error_text(
        &s.delete_event(params(json!({"event_id": id})))
            .await
            .unwrap(),
    );
    assert!(e.contains("would email them"), "{e}");
    let e = error_text(
        &s.update_event(params(json!({"event_id": id})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
    let st = fake.state();
    assert_eq!(st.count("PUT") + st.count("DELETE"), 0);
}

#[tokio::test]
async fn delete_series_occurrence_and_following() {
    let fake = fake().await;
    let s = server(&fake, false);
    let standup = "/1234567890/calendars/work/standup-1.ics";
    let v = json(
        &s.delete_event(params(json!({
            "event_id": standup, "scope": "occurrence", "occurrence_start": "2026-11-02T09:00:00+00:00"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["series_deleted"], false);
    let v = json(
        &s.delete_event(params(json!({
            "event_id": standup, "scope": "this_and_following", "occurrence_start": "2026-11-16T09:00:00+00:00"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["series_deleted"], false);
    let standups: Vec<String> = summaries(&fake, "2026-10-26", "2026-12-31")
        .await
        .into_iter()
        .filter(|s| s.contains("Standup"))
        .collect();
    assert_eq!(
        standups,
        [
            "2026-10-26T10:00:00+00:00 Standup (late)",
            "2026-11-09T09:00:00+00:00 Standup"
        ]
    );
    let v = json(
        &s.delete_event(params(json!({
            "event_id": standup, "scope": "this_and_following", "occurrence_start": "2026-10-05T09:00:00+01:00"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["series_deleted"], true);
    assert!(fake.state().resource("work", "standup-1.ics").is_none());

    let etag = fake.state().resource("home", "dentist-1.ics").unwrap().etag;
    json(
        &s.delete_event(params(
            json!({"event_id": "/1234567890/calendars/home/dentist-1.ics"}),
        ))
        .await
        .unwrap(),
    );
    assert!(fake.state().resource("home", "dentist-1.ics").is_none());
    let st = fake.state();
    let del = st.log.iter().rfind(|l| l.method == "DELETE").unwrap();
    assert_eq!(del.if_match.as_deref(), Some(etag.as_str()));
}

#[tokio::test]
async fn deleting_a_received_invitation_is_refused_when_invites_are_off() {
    let fake = fake().await;
    let e = error_text(
        &server(&fake, false)
            .delete_event(params(
                json!({"event_id": "/1234567890/calendars/work/invite-1.ics"}),
            ))
            .await
            .unwrap(),
    );
    assert!(e.contains("send them a reply"), "{e}");
    assert_eq!(fake.state().count("DELETE"), 0);
}

#[tokio::test]
async fn hostile_ids_are_refused_before_any_request() {
    let fake = fake().await;
    let s = server(&fake, false);
    for id in [
        "https://evil.example.com/1234567890/calendars/home/dentist-1.ics",
        "/1234567890/calendars/home/..%2Fwork/standup-1.ics",
    ] {
        let e = error_text(
            &s.update_event(params(json!({"event_id": id, "summary": "x"})))
                .await
                .unwrap(),
        );
        assert!(e.starts_with("validation:"), "{e}");
        let e = error_text(
            &s.delete_event(params(json!({"event_id": id})))
                .await
                .unwrap(),
        );
        assert!(e.starts_with("validation:"), "{e}");
    }
    let st = fake.state();
    assert_eq!(st.count("GET") + st.count("PUT") + st.count("DELETE"), 0);
}
