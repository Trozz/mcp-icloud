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
