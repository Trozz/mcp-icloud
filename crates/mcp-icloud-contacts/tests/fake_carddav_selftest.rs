//! The fake must answer like iCloud did when probed, or the other tests prove nothing.
mod support;
use mcp_icloud_dav::DavError;
use mcp_icloud_dav::client::Precondition;
use mcp_icloud_dav::xml::{addressbook_multiget, sync_collection};
use support::cards::contact;
use support::fake_carddav::{BOOK, FakeCardDav, FakeState};

#[tokio::test]
async fn writes_behave_like_icloud() {
    let fake = FakeCardDav::start(FakeState::icloud_like()).await;
    let client = fake.client();
    let url = fake.url.join(&format!("{BOOK}A.vcf")).unwrap();
    let body = contact("A", "Ann", "Example", &[]);
    let etag = client
        .put(&url, body.clone(), Precondition::IfNoneMatch)
        .await
        .unwrap()
        .expect("a create returns an ETag");
    assert_eq!(fake.state().card("A.vcf").unwrap().body, body);
    let again = client
        .put(&url, body.clone(), Precondition::IfNoneMatch)
        .await;
    assert!(matches!(again, Err(DavError::Forbidden(_))), "{again:?}");
    let stale = client
        .put(
            &url,
            body.clone(),
            Precondition::IfMatch("\"stale\"".into()),
        )
        .await;
    assert_eq!(stale, Err(DavError::Conflict));
    let fresh = client
        .put(&url, body, Precondition::IfMatch(etag))
        .await
        .unwrap();
    assert!(fresh.is_some());
    client.delete(&url, "\"stale\"").await.unwrap();
    assert!(
        fake.state().card("A.vcf").is_none(),
        "iCloud ignores If-Match on DELETE"
    );
    client.delete(&url, "\"stale\"").await.unwrap();
}

#[tokio::test]
async fn sync_tokens_and_multiget_behave_like_icloud() {
    let mut state = FakeState::icloud_like();
    state.put("A.vcf", &contact("A", "Ann", "Example", &[]));
    state.put("B.vcf", &contact("B", "Bob", "Example", &[]));
    let fake = FakeCardDav::start(state).await;
    let client = fake.client();
    let book = fake.url.join(BOOK).unwrap();

    let full = client
        .sync_report(&book, sync_collection(None))
        .await
        .unwrap();
    let hrefs: Vec<&str> = full.responses.iter().map(|r| r.href.as_str()).collect();
    assert_eq!(
        hrefs,
        [
            BOOK.trim_end_matches('/'),
            &format!("{BOOK}A.vcf"),
            &format!("{BOOK}B.vcf")
        ]
    );
    let token = full.sync_token.expect("a token");

    let none = client
        .sync_report(&book, sync_collection(Some(&token)))
        .await
        .unwrap();
    assert!(none.responses.is_empty());

    fake.state()
        .put("A.vcf", &contact("A", "Ann", "Changed", &[]));
    fake.state().remove("B.vcf");
    let delta = client
        .sync_report(&book, sync_collection(Some(&token)))
        .await
        .unwrap();
    let seen: Vec<(String, Option<u16>)> = delta
        .responses
        .iter()
        .map(|r| (r.href.clone(), r.status))
        .collect();
    assert_eq!(
        seen,
        [
            (format!("{BOOK}A.vcf"), None),
            (format!("{BOOK}B.vcf"), Some(404))
        ]
    );

    fake.state().expire_tokens();
    let expired = client
        .sync_report(&book, sync_collection(Some(&token)))
        .await;
    assert_eq!(expired.unwrap_err(), DavError::Conflict);

    let got = client
        .report(
            &book,
            addressbook_multiget(&[format!("{BOOK}A.vcf"), format!("{BOOK}gone.vcf")]),
        )
        .await
        .unwrap();
    assert_eq!(got.len(), 2);
    assert!(
        got[0]
            .props
            .address_data
            .as_deref()
            .unwrap()
            .contains("N:Changed;Ann;;;")
    );
    assert_eq!(got[1].status, Some(404));
}
