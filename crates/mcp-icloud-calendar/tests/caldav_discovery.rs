//! Discovery against the in-memory CalDAV fake.
mod support;
use mcp_icloud_calendar::caldav::client::{CaldavClient, CaldavSettings};
use mcp_icloud_calendar::caldav::discovery::{Directory, discover};
use secrecy::SecretString;
use std::sync::Arc;
use support::fake_caldav::{EMAIL, FakeCalDav, FakeState, HOME, PASSWORD};

fn client(fake: &FakeCalDav, password: &str) -> CaldavClient {
    CaldavClient::new(CaldavSettings::new(
        fake.url.clone(),
        EMAIL.into(),
        SecretString::from(password.to_string()),
    ))
    .unwrap()
}

#[tokio::test]
async fn finds_event_calendars_home_and_my_addresses() {
    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let a = discover(&client(&fake, PASSWORD), EMAIL).await.unwrap();
    assert_eq!(a.home, fake.url.join(HOME).unwrap());
    let names: Vec<(&str, bool)> = a
        .calendars
        .iter()
        .map(|c| (c.name.as_str(), c.writable))
        .collect();
    assert_eq!(names, [("Holidays", false), ("Home", true), ("Work", true)]);
    let home = a.calendar("Home").unwrap();
    assert_eq!(home.id, "/1234567890/calendars/home/");
    assert_eq!(home.color.as_deref(), Some("#1BADF8FF"));
    assert!(a.me.matches("mailto:user@example.com"));
    assert!(a.me.matches("mailto:USER.ALIAS@example.org"));
    assert!(a.me.matches("urn:uuid:1234567890"));
    assert!(!a.me.matches("mailto:someone@example.net"));

    let s = fake.state();
    let depths: Vec<Option<&str>> = s.log.iter().map(|l| l.depth.as_deref()).collect();
    assert_eq!(depths, [Some("0"), Some("0"), Some("1")]);
    assert!(s.log.iter().all(|l| l.authorized));
}

#[tokio::test]
async fn follows_a_redirect_on_the_root() {
    let mut st = FakeState::icloud_like();
    st.redirect_root = true;
    let fake = FakeCalDav::start(st).await;
    let a = discover(&client(&fake, PASSWORD), EMAIL).await.unwrap();
    assert_eq!(a.calendars.len(), 3);
    let paths: Vec<String> = fake.state().log.iter().map(|l| l.path.clone()).collect();
    assert_eq!(paths[..2], ["/".to_string(), "/root/".to_string()]);
}

#[tokio::test]
async fn refuses_a_calendar_home_on_another_origin() {
    let other = FakeCalDav::start(FakeState::icloud_like()).await;
    let mut st = FakeState::icloud_like();
    st.home_origin = Some(other.origin());
    let fake = FakeCalDav::start(st).await;
    let e = discover(&client(&fake, PASSWORD), EMAIL).await.unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert!(e.to_string().contains("not a trusted CalDAV origin"), "{e}");
    assert!(
        other.state().log.is_empty(),
        "no request may reach the other origin"
    );
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let e = discover(&client(&fake, "wrong"), EMAIL).await.unwrap_err();
    assert_eq!(e.kind(), "auth");
}

#[tokio::test]
async fn the_directory_caches_until_refreshed() {
    let fake = FakeCalDav::start(FakeState::icloud_like()).await;
    let dir = Directory::new(Arc::new(client(&fake, PASSWORD)), EMAIL.into());
    let a = dir.account().await.unwrap();
    let b = dir.account().await.unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(fake.state().count("PROPFIND"), 3);
    dir.refresh().await.unwrap();
    assert_eq!(fake.state().count("PROPFIND"), 6);
    dir.invalidate().await;
    dir.account().await.unwrap();
    assert_eq!(fake.state().count("PROPFIND"), 9);
}
