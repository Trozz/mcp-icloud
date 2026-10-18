//! create_calendar, rename_calendar and delete_calendar against the fake CalDAV server.
mod support;
use serde_json::{Value, json};
use support::events::*;
use support::fake_caldav::{FakeCalDav, FakeState, HOME};
use support::tools::{error_text, json, params, server};

async fn fake() -> FakeCalDav {
    let mut st = FakeState::icloud_like();
    st.put("work", "standup-1.ics", STANDUP);
    FakeCalDav::start(st).await
}

/// Requests that change calendars: none may be sent after a refused call.
fn writes(fake: &FakeCalDav) -> usize {
    let st = fake.state();
    st.count("MKCALENDAR") + st.count("PROPPATCH") + st.count("DELETE")
}

#[tokio::test]
async fn create_makes_an_event_calendar_and_lists_it() {
    let fake = fake().await;
    let s = server(&fake, false);
    let v = json(
        &s.create_calendar(params(
            json!({"name": " ZZ MCP Probe ", "color": "#1badf8"}),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(v["created"], true);
    let cal = &v["calendar"];
    assert_eq!(cal["name"], "ZZ MCP Probe");
    assert_eq!(cal["color"], "#1BADF8FF");
    assert_eq!(cal["writable"], true);
    let id = cal["id"].as_str().unwrap().to_string();
    let slug = id
        .strip_prefix(HOME)
        .and_then(|r| r.strip_suffix('/'))
        .unwrap()
        .to_string();
    assert_eq!(slug.len(), 36, "{id}");
    {
        let st = fake.state();
        let mk = st.log.iter().find(|l| l.method == "MKCALENDAR").unwrap();
        assert_eq!(mk.path, id);
        assert!(
            mk.body
                .contains("<d:displayname>ZZ MCP Probe</d:displayname>"),
            "{}",
            mk.body
        );
        assert!(
            mk.body
                .contains("<a:calendar-color>#1BADF8FF</a:calendar-color>"),
            "{}",
            mk.body
        );
        assert_eq!(st.calendars[&slug].components, ["VEVENT"]);
    }
    let listed = json(&s.list_calendars().await.unwrap());
    let shown = listed["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id.as_str())
        .cloned();
    assert_eq!(shown, Some(cal.clone()));
}

#[tokio::test]
async fn names_with_xml_characters_round_trip() {
    let fake = fake().await;
    let v = json(
        &server(&fake, false)
            .create_calendar(params(json!({"name": "Kids & <School>"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["calendar"]["name"], "Kids & <School>");
    assert_eq!(v["calendar"]["color"], Value::Null);
}

#[tokio::test]
async fn create_validates_before_any_write() {
    let fake = fake().await;
    let s = server(&fake, false);
    for args in [
        json!({"name": "   "}),
        json!({"name": "Tab\there"}),
        json!({"name": "Line\nbreak"}),
        json!({"name": "Garden", "color": "red"}),
        json!({"name": "Garden", "color": "1BADF8"}),
        json!({"name": "Garden", "color": "#12345"}),
        json!({"name": "Garden", "color": "#1BADF8FF"}),
    ] {
        let e = error_text(&s.create_calendar(params(args.clone())).await.unwrap());
        assert!(e.starts_with("validation:"), "{args}: {e}");
    }
    for name in ["Work", "work", " HOLIDAYS "] {
        let e = error_text(
            &s.create_calendar(params(json!({"name": name})))
                .await
                .unwrap(),
        );
        assert!(e.starts_with("conflict: a calendar named"), "{name}: {e}");
    }
    assert_eq!(writes(&fake), 0);
}

#[tokio::test]
async fn rename_changes_the_name_and_colour() {
    let fake = fake().await;
    let s = server(&fake, false);
    let v = json(
        &s.rename_calendar(params(json!({
            "calendar": "Work", "new_name": "Office", "color": "#00ff00"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["updated"], true);
    assert_eq!(
        v["calendar"],
        json!({"id": "/1234567890/calendars/work/", "name": "Office", "color": "#00FF00FF", "writable": true})
    );
    let st = fake.state();
    let patch = st.log.iter().find(|l| l.method == "PROPPATCH").unwrap();
    assert_eq!(patch.path, "/1234567890/calendars/work/");
    assert!(
        patch.body.contains("<d:displayname>Office</d:displayname>"),
        "{}",
        patch.body
    );
    assert_eq!(st.resource_names("work"), ["standup-1.ics"]);
}

#[tokio::test]
async fn rename_can_change_only_the_colour_or_only_the_case() {
    let fake = fake().await;
    let s = server(&fake, false);
    let v = json(
        &s.rename_calendar(params(json!({
            "calendar": "/1234567890/calendars/home/", "color": "#FF2968"
        })))
        .await
        .unwrap(),
    );
    assert_eq!(v["calendar"]["name"], "Home");
    assert_eq!(v["calendar"]["color"], "#FF2968FF");
    let body = fake
        .state()
        .log
        .iter()
        .find(|l| l.method == "PROPPATCH")
        .unwrap()
        .body
        .clone();
    assert!(!body.contains("displayname"), "{body}");
    let v = json(
        &s.rename_calendar(params(json!({"calendar": "work", "new_name": "WORK"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["calendar"]["name"], "WORK");
}

#[tokio::test]
async fn rename_validates_and_refuses_before_any_write() {
    let fake = fake().await;
    let s = server(&fake, false);
    let cases = [
        (json!({"calendar": "Work"}), "validation:"),
        (json!({"calendar": "Work", "new_name": "  "}), "validation:"),
        (
            json!({"calendar": "Work", "new_name": "a\tb"}),
            "validation:",
        ),
        (json!({"calendar": "Work", "color": "blue"}), "validation:"),
        (json!({"calendar": "Work", "new_name": "home"}), "conflict:"),
        (
            json!({"calendar": "Holidays", "new_name": "Days off"}),
            "read_only:",
        ),
        (
            json!({"calendar": "Garden", "new_name": "Yard"}),
            "not_found:",
        ),
        (
            json!({"calendar": "/1234567890/calendars/../principal/", "new_name": "x"}),
            "validation:",
        ),
    ];
    for (args, kind) in cases {
        let e = error_text(&s.rename_calendar(params(args.clone())).await.unwrap());
        assert!(e.starts_with(kind), "{args}: {e}");
    }
    assert_eq!(writes(&fake), 0);
}

#[tokio::test]
async fn a_change_icloud_did_not_apply_is_an_upstream_error() {
    let mut st = FakeState::icloud_like();
    st.ignore_proppatch = true;
    let fake = FakeCalDav::start(st).await;
    let e = error_text(
        &server(&fake, false)
            .rename_calendar(params(json!({"calendar": "Work", "new_name": "Office"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        e,
        "upstream: iCloud error: iCloud accepted the change but did not apply it"
    );
}
