//! The fake must behave like iCloud for the semantics the tools rely on.
mod support;
use mcp_icloud_calendar::caldav::caldav_settings;
use mcp_icloud_calendar::caldav::xml::calendar_query;
use mcp_icloud_dav::client::{DavClient, Precondition};
use secrecy::SecretString;
use support::fake_caldav::{EMAIL, FakeCalDav, FakeState, PASSWORD};

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example Corp//Test//EN\r\nBEGIN:VEVENT\r\nUID:self-test\r\nDTSTAMP:20261001T080000Z\r\nDTSTART:20261020T090000Z\r\nDTEND:20261020T100000Z\r\nSUMMARY:Self test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn client(fake: &FakeCalDav) -> DavClient {
    DavClient::new(caldav_settings(
        fake.url.clone(),
        EMAIL.into(),
        SecretString::from(PASSWORD),
    ))
    .unwrap()
}

#[tokio::test]
async fn conditional_writes_follow_etags() {
    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let c = client(&fake);
    let url = fake
        .url
        .join(&FakeCalDav::event_id("home", "self-test.ics"))
        .unwrap();
    let e1 = c
        .put(&url, EVENT.into(), Precondition::IfNoneMatch)
        .await
        .unwrap()
        .unwrap();
    let again = c.put(&url, EVENT.into(), Precondition::IfNoneMatch).await;
    assert_eq!(again.unwrap_err().kind(), "conflict");
    let got = c.get(&url).await.unwrap();
    assert_eq!(got.etag.as_deref(), Some(e1.as_str()));
    assert_eq!(got.body, EVENT);
    let stale = c
        .put(&url, EVENT.into(), Precondition::IfMatch("\"nope\"".into()))
        .await;
    assert_eq!(stale.unwrap_err().kind(), "conflict");
    let e2 = c
        .put(&url, EVENT.into(), Precondition::IfMatch(e1.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(e1, e2);
    assert_eq!(c.delete(&url, &e1).await.unwrap_err().kind(), "conflict");
    c.delete(&url, &e2).await.unwrap();
    assert_eq!(c.get(&url).await.unwrap_err().kind(), "not_found");
}

#[tokio::test]
async fn read_only_calendars_refuse_writes() {
    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let url = fake
        .url
        .join(&FakeCalDav::event_id("holidays", "x.ics"))
        .unwrap();
    let e = client(&fake)
        .put(&url, EVENT.into(), Precondition::IfNoneMatch)
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "read_only");
}

#[tokio::test]
async fn report_returns_every_resource_with_data_and_etag() {
    let mut st = FakeState::icloud_like();
    let etag = st.put("work", "a.ics", EVENT);
    let fake = FakeCalDav::start(st).await;
    let url = fake.url.join("/1234567890/calendars/work/").unwrap();
    let now = chrono::Utc::now();
    let r = client(&fake)
        .report(&url, calendar_query(now, now))
        .await
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].href, "/1234567890/calendars/work/a.ics");
    assert_eq!(r[0].props.etag.as_deref(), Some(etag.as_str()));
    assert_eq!(r[0].props.calendar_data.as_deref(), Some(EVENT.trim()));
}

#[tokio::test]
async fn injected_failures_are_served_once() {
    let mut st = FakeState::icloud_like();
    st.put("home", "a.ics", EVENT);
    st.fail.push_back(("PUT", 507));
    let fake = FakeCalDav::start(st).await;
    let c = client(&fake);
    let url = fake
        .url
        .join(&FakeCalDav::event_id("home", "b.ics"))
        .unwrap();
    let e = c
        .put(&url, EVENT.into(), Precondition::IfNoneMatch)
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "quota");
    c.put(&url, EVENT.into(), Precondition::IfNoneMatch)
        .await
        .unwrap();
    assert_eq!(fake.state().resource_names("home"), ["a.ics", "b.ics"]);
}
