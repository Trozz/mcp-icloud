mod support;
use serde_json::{Value, json};
use std::time::Duration;
use support::events::*;
use support::fake_caldav::{FakeCalDav, FakeCalendar, FakeState};
use support::tools::{error_text, json, params, server};

async fn fake() -> FakeCalDav {
    let mut st = FakeState::icloud_like();
    st.put("home", "dentist-1.ics", DENTIST);
    st.put("home", "christmas-1.ics", CHRISTMAS);
    st.put("home", "lunch-1.ics", LUNCH_HOLD);
    st.put("home", "broken-1.ics", BROKEN);
    st.put("work", "standup-1.ics", STANDUP);
    st.put("work", "invite-1.ics", INVITE);
    st.put("work", "declined-1.ics", DECLINED);
    FakeCalDav::start(st).await
}

fn summaries(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["summary"].as_str().unwrap_or_default().to_string())
        .collect()
}

const WEEK: (&str, &str) = ("2026-10-19", "2026-10-27");

#[tokio::test]
async fn read_tools_are_registered() {
    let fake = fake().await;
    let names = server(&fake, false).tool_names();
    for n in [
        "find_free_time",
        "get_event",
        "get_timezone",
        "list_calendars",
        "list_events",
        "set_timezone",
    ] {
        assert!(names.contains(&n.to_string()), "{names:?}");
    }
}

#[tokio::test]
async fn list_calendars_shows_event_calendars_only() {
    let fake = fake().await;
    let v = json(&server(&fake, false).list_calendars().await.unwrap());
    assert_eq!(
        v["calendars"],
        json!([
            {"id": "/1234567890/calendars/holidays/", "name": "Holidays", "color": "#1BADF8FF", "writable": false},
            {"id": "/1234567890/calendars/home/", "name": "Home", "color": "#1BADF8FF", "writable": true},
            {"id": "/1234567890/calendars/work/", "name": "Work", "color": "#1BADF8FF", "writable": true},
        ])
    );
}

#[tokio::test]
async fn list_events_expands_series_applies_exceptions_and_sorts() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .list_events(params(json!({"start": WEEK.0, "end": WEEK.1})))
            .await
            .unwrap(),
    );
    assert_eq!(v["timezone"], "Europe/London");
    assert_eq!(v["truncated"], false);
    assert_eq!(
        summaries(&v),
        [
            "Dentist",
            "Lunch hold",
            "Vendor pitch",
            "Budget review",
            "Standup (late)"
        ],
        "19 Oct is excluded, 26 Oct is moved, the broken resource is skipped"
    );
    let dentist = &v["events"][0];
    assert_eq!(
        dentist["event_id"],
        "/1234567890/calendars/home/dentist-1.ics"
    );
    assert_eq!(dentist["calendar"], "Home");
    assert_eq!(dentist["start"], "2026-10-20T09:00:00+01:00");
    assert_eq!(dentist["end"], "2026-10-20T10:00:00+01:00");
    assert_eq!(dentist["all_day"], false);
    assert_eq!(dentist["recurring"], false);
    assert!(dentist.get("occurrence_start").is_none());
    let late = &v["events"][4];
    assert_eq!(late["occurrence_start"], "2026-10-26T09:00:00+00:00");
    assert_eq!(late["start"], "2026-10-26T10:00:00+00:00");
    assert_eq!(late["recurring"], true);
    let invite = &v["events"][3];
    assert_eq!(invite["my_status"], "needs_action");
}

#[tokio::test]
async fn list_events_filters_by_query_calendar_and_needs_response() {
    let fake = fake().await;
    let s = server(&fake, false);
    let v = json(
        &s.list_events(params(
            json!({"start": WEEK.0, "end": WEEK.1, "query": "CLINIC"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(summaries(&v), ["Dentist"]);
    let v = json(
        &s.list_events(params(
            json!({"start": WEEK.0, "end": WEEK.1, "calendars": ["work"]}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(
        summaries(&v),
        ["Vendor pitch", "Budget review", "Standup (late)"]
    );
    let v = json(
        &s.list_events(params(
            json!({"start": WEEK.0, "end": WEEK.1, "needs_response": true}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(summaries(&v), ["Budget review"]);
    let e = error_text(
        &s.list_events(params(json!({"calendars": ["Birthdays"]})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("not_found:"), "{e}");
}

#[tokio::test]
async fn all_day_events_are_dates_with_an_inclusive_end() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .list_events(params(
                json!({"start": "2026-12-01", "end": "2027-01-01", "query": "christmas"}),
            ))
            .await
            .unwrap(),
    );
    let ev = &v["events"][0];
    assert_eq!(ev["summary"], "Christmas break");
    assert_eq!(ev["all_day"], true);
    assert_eq!(ev["start_date"], "2026-12-24");
    assert_eq!(ev["end_date"], "2026-12-26");
}

#[tokio::test]
async fn timezone_argument_and_override_change_the_output_zone() {
    let fake = fake().await;
    let s = server(&fake, false);
    let args =
        json!({"start": WEEK.0, "end": WEEK.1, "query": "dentist", "timezone": "America/New_York"});
    let v = json(&s.list_events(params(args)).await.unwrap());
    assert_eq!(v["events"][0]["start"], "2026-10-20T04:00:00-04:00");

    let tz = json(
        &s.set_timezone(params(json!({"zone": "Asia/Tokyo"})))
            .await
            .unwrap(),
    );
    assert_eq!(tz["timezone"], "Asia/Tokyo");
    assert_eq!(tz["source"], "override");
    assert_eq!(tz["default"], "Europe/London");
    let v = json(
        &s.list_events(params(
            json!({"start": "2026-10-20T00:00:00+01:00", "end": "2026-10-21T00:00:00+01:00", "query": "dentist"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["events"][0]["start"], "2026-10-20T17:00:00+09:00");
    assert_eq!(
        v["events"][0]["event_timezone"], "Europe/London",
        "the event's own zone is shown when it differs"
    );

    let tz = json(&s.set_timezone(params(json!({"zone": null}))).await.unwrap());
    assert_eq!(tz["timezone"], "Europe/London");
    assert_eq!(tz["source"], "config");
    let tz = json(&s.get_timezone().await.unwrap());
    assert_eq!(tz["source"], "config");

    let e = error_text(
        &s.set_timezone(params(json!({"zone": "Mars/Olympus"})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
}

#[tokio::test]
async fn ranges_over_366_days_are_refused() {
    let fake = fake().await;
    let e = error_text(
        &server(&fake, false)
            .list_events(params(json!({"start": "2026-01-01", "end": "2027-01-03"})))
            .await
            .unwrap(),
    );
    assert!(
        e.starts_with("validation:") && e.contains("366 days"),
        "{e}"
    );
}

#[tokio::test]
async fn listings_stop_at_500_occurrences() {
    let mut st = FakeState::icloud_like();
    st.put("home", "daily-a.ics", &daily("daily-a"));
    st.put("work", "daily-b.ics", &daily("daily-b"));
    let fake = FakeCalDav::start(st).await;
    let v = json(
        &server(&fake, false)
            .list_events(params(json!({"start": "2026-01-01", "end": "2027-01-01"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["events"].as_array().unwrap().len(), 500);
    assert_eq!(v["truncated"], true);
}

#[tokio::test]
async fn at_most_four_reports_run_at_once() {
    let mut st = FakeState::icloud_like();
    for i in 0..6 {
        st.calendars.insert(
            format!("extra{i}"),
            FakeCalendar::events(&format!("Extra {i}"), true),
        );
    }
    st.report_delay = Duration::from_millis(150);
    let fake = FakeCalDav::start(st).await;
    json(
        &server(&fake, false)
            .list_events(params(json!({"start": WEEK.0, "end": WEEK.1})))
            .await
            .unwrap(),
    );
    let s = fake.state();
    assert_eq!(s.count("REPORT"), 9);
    assert_eq!(s.max_reports_in_flight, 4);
}

#[tokio::test]
async fn a_vanished_calendar_refreshes_discovery() {
    let fake = fake().await;
    let s = server(&fake, false);
    json(
        &s.list_events(params(json!({"start": WEEK.0, "end": WEEK.1})))
            .await
            .unwrap(),
    );
    assert_eq!(fake.state().count("PROPFIND"), 3);
    fake.state().fail.push_back(("REPORT", 404));
    let e = error_text(
        &s.list_events(params(json!({"start": WEEK.0, "end": WEEK.1})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("not_found:"), "{e}");
    json(
        &s.list_events(params(json!({"start": WEEK.0, "end": WEEK.1})))
            .await
            .unwrap(),
    );
    assert_eq!(fake.state().count("PROPFIND"), 6, "discovery ran again");
}

#[tokio::test]
async fn get_event_shows_the_series_and_its_exceptions() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .get_event(params(
                json!({"event_id": "/1234567890/calendars/work/standup-1.ics"}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(v["summary"], "Standup");
    assert_eq!(v["calendar"], "Work");
    assert_eq!(v["start"], "2026-10-05T09:00:00+01:00");
    assert_eq!(v["recurrence"]["text"], "every week on Monday, 10 times");
    assert_eq!(v["recurrence"]["rrule"], "FREQ=WEEKLY;BYDAY=MO;COUNT=10");
    assert_eq!(
        v["excluded_occurrences"],
        json!(["2026-10-19T09:00:00+01:00"])
    );
    assert_eq!(
        v["changed_occurrences"],
        json!(["2026-10-26T09:00:00+00:00"])
    );
}

#[tokio::test]
async fn get_event_reads_single_occurrences() {
    let fake = fake().await;
    let s = server(&fake, false);
    let id = "/1234567890/calendars/work/standup-1.ics";
    let v = json(
        &s.get_event(params(
            json!({"event_id": id, "occurrence_start": "2026-10-26T09:00:00+00:00"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["summary"], "Standup (late)");
    assert_eq!(v["start"], "2026-10-26T10:00:00+00:00");
    let v = json(
        &s.get_event(params(
            json!({"event_id": id, "occurrence_start": "2026-11-02T09:00:00Z"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["summary"], "Standup");
    assert_eq!(v["start"], "2026-11-02T09:00:00+00:00");
    assert_eq!(v["end"], "2026-11-02T09:15:00+00:00");
    let e = error_text(
        &s.get_event(params(
            json!({"event_id": id, "occurrence_start": "2026-11-03T09:00:00Z"}),
        ))
        .await
        .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
}

#[tokio::test]
async fn get_event_shows_attendees_alerts_and_inclusive_all_day_end() {
    let fake = fake().await;
    let s = server(&fake, false);
    let v = json(
        &s.get_event(params(
            json!({"event_id": "/1234567890/calendars/work/invite-1.ics"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["organizer"]["email"], "olive@example.net");
    assert_eq!(v["organizer"]["is_me"], false);
    assert_eq!(v["my_status"], "needs_action");
    let me: Vec<&Value> = v["attendees"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["is_me"] == true)
        .collect();
    assert_eq!(me.len(), 1);

    let v = json(
        &s.get_event(params(
            json!({"event_id": "/1234567890/calendars/home/dentist-1.ics"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["alerts_minutes_before"], json!([30]));
    assert_eq!(v["description"], "Bring the referral letter");
    assert_eq!(v["busy"], true);

    let v = json(
        &s.get_event(params(
            json!({"event_id": "/1234567890/calendars/home/christmas-1.ics"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["start_date"], "2026-12-24");
    assert_eq!(v["end_date"], "2026-12-26");
    assert_eq!(v["busy"], false);
}

#[tokio::test]
async fn get_event_refuses_ids_outside_the_calendars() {
    let fake = fake().await;
    let s = server(&fake, false);
    for id in [
        "https://evil.example.com/1234567890/calendars/home/dentist-1.ics",
        "/1234567890/calendars/home/../work/standup-1.ics",
        "/1234567890/calendars/home/%2e%2e/work/standup-1.ics",
        "/1234567890/principal/",
    ] {
        let e = error_text(&s.get_event(params(json!({"event_id": id}))).await.unwrap());
        assert!(e.starts_with("validation:"), "{id}: {e}");
    }
    let before = fake.state().count("GET");
    assert_eq!(before, 0, "nothing may be fetched for a refused id");
    let absolute = format!("{}1234567890/calendars/home/dentist-1.ics", fake.url);
    let v = json(
        &s.get_event(params(json!({"event_id": absolute})))
            .await
            .unwrap(),
    );
    assert_eq!(v["summary"], "Dentist");
    assert_eq!(v["event_id"], "/1234567890/calendars/home/dentist-1.ics");
}

#[tokio::test]
async fn find_free_time_skips_free_cancelled_and_declined_events() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .find_free_time(params(json!({
                "start": "2026-10-20",
                "end": "2026-10-21",
                "duration_minutes": 30,
                "working_hours": {"start": "08:00", "end": "17:00"}
            })))
            .await
            .unwrap(),
    );
    assert_eq!(
        v["free"],
        json!([
            {"start": "2026-10-20T08:00:00+01:00", "end": "2026-10-20T09:00:00+01:00", "minutes": 60},
            {"start": "2026-10-20T10:00:00+01:00", "end": "2026-10-20T17:00:00+01:00", "minutes": 420},
        ]),
        "the lunch hold is free and the vendor pitch was declined"
    );
    let e = error_text(
        &server(&fake, false)
            .find_free_time(params(json!({
                "start": "2026-10-20", "end": "2026-10-21", "duration_minutes": 0
            })))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
}

#[tokio::test]
async fn find_free_time_reports_truncated_when_a_series_expansion_is_capped() {
    let hourly = daily("hourly").replace("FREQ=DAILY", "FREQ=HOURLY");
    let mut st = FakeState::icloud_like();
    st.put("home", "hourly.ics", &hourly);
    let fake = FakeCalDav::start(st).await;
    let v = json(
        &server(&fake, false)
            .find_free_time(params(json!({
                "start": "2026-01-01",
                "end": "2026-03-01",
                "duration_minutes": 15
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["truncated"], true);
}
