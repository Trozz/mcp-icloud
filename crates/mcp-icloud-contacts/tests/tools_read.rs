mod support;
use rmcp::ServerHandler;
use serde_json::json;
use support::cards::{COMPANY_UID, GROUP_UID, PERSON_UID, seed};
use support::fake_carddav::{FakeCardDav, FakeState};
use support::tools::{error_text, json, params, server};

async fn seeded() -> FakeCardDav {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    FakeCardDav::start(state).await
}

#[tokio::test]
async fn search_without_a_query_lists_everyone_by_name() {
    let fake = seeded().await;
    let v = json(
        &server(&fake)
            .search_contacts(params(json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(v["total"], 2);
    assert_eq!(v["offset"], 0);
    assert_eq!(
        v["contacts"],
        json!([
            {
                "id": PERSON_UID,
                "display_name": "Dr. Alex J. Example Jr.",
                "organization": "Example Widgets Ltd",
                "emails": ["alex@example.com", "alex.work@example.org"],
                "phones": ["+44 7700 900123", "07700 900456", "+1 202-555-0123"],
                "is_me": true
            },
            {
                "id": COMPANY_UID,
                "display_name": "Example Plumbing Co",
                "organization": "Example Plumbing Co",
                "emails": [],
                "phones": ["+44 7700 900789"],
                "is_me": false
            }
        ])
    );
}

#[tokio::test]
async fn search_finds_words_and_phone_numbers() {
    let fake = seeded().await;
    let s = server(&fake);
    for q in [
        "alex widgets",
        "ÁLEX",
        "07700 900123",
        "+447700900123",
        "plumbing",
    ] {
        let v = json(
            &s.search_contacts(params(json!({"query": q})))
                .await
                .unwrap(),
        );
        assert_eq!(v["total"], 1, "{q}");
    }
    let v = json(
        &s.search_contacts(params(json!({"query": "nobody here"})))
            .await
            .unwrap(),
    );
    assert_eq!(v["total"], 0);
    assert_eq!(v["contacts"], json!([]));
}

#[tokio::test]
async fn search_pages_and_checks_its_limit() {
    let fake = seeded().await;
    let s = server(&fake);
    let v = json(
        &s.search_contacts(params(json!({"limit": 1, "offset": 1})))
            .await
            .unwrap(),
    );
    assert_eq!(v["total"], 2);
    assert_eq!(v["contacts"][0]["id"], COMPANY_UID);
    assert_eq!(v["contacts"].as_array().unwrap().len(), 1);
    for limit in [0, 101] {
        let e = error_text(
            &s.search_contacts(params(json!({"limit": limit})))
                .await
                .unwrap(),
        );
        assert!(
            e.starts_with("validation: limit must be between 1 and 100"),
            "{e}"
        );
    }
}

#[tokio::test]
async fn search_within_a_group_ignores_members_that_no_longer_exist() {
    let fake = seeded().await;
    let s = server(&fake);
    let v = json(
        &s.search_contacts(params(json!({"group_id": GROUP_UID})))
            .await
            .unwrap(),
    );
    assert_eq!(v["total"], 2);
    let e = error_text(
        &s.search_contacts(params(json!({"group_id": PERSON_UID})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
}

#[tokio::test]
async fn get_contact_shows_every_field_and_the_groups() {
    let fake = seeded().await;
    let v = json(
        &server(&fake)
            .get_contact(params(json!({"id": PERSON_UID})))
            .await
            .unwrap(),
    );
    assert_eq!(v["id"], PERSON_UID);
    assert_eq!(v["is_me"], true);
    assert_eq!(v["given_name"], "Alex");
    assert_eq!(v["department"], "Research");
    assert_eq!(
        v["emails"][0],
        json!({"value": "alex@example.com", "label": "home", "preferred": true})
    );
    assert_eq!(
        v["emails"][1],
        json!({"value": "alex.work@example.org", "label": "office"})
    );
    assert_eq!(
        v["phones"][2],
        json!({"value": "+1 202-555-0123", "label": "boat"})
    );
    assert_eq!(v["addresses"][0]["street"], "1 Example Street\nFlat 2");
    assert_eq!(v["urls"][0]["label"], "home page");
    assert_eq!(v["birthday"], "--05-12");
    assert_eq!(
        v["dates"],
        json!([{"date": "2010-06-01", "label": "anniversary"}])
    );
    assert_eq!(
        v["related"][0],
        json!({"name": "Sam Example", "label": "mother"})
    );
    assert_eq!(v["social"][0]["label"], "twitter");
    assert_eq!(v["im"][0]["value"], "skype:alex.example");
    assert_eq!(v["note"], "Met at the conference, 2019.\nLikes tea.");
    assert_eq!(v["groups"], json!([{"id": GROUP_UID, "name": "Book Club"}]));
    assert_eq!(v["has_photo"], true);
    assert_eq!(v["updated"], "2026-10-17T18:23:20Z");
    assert!(v.get("suffix").is_some() && v.get("nickname").is_some());
}

#[tokio::test]
async fn get_contact_refuses_unknown_ids_and_group_ids() {
    let fake = seeded().await;
    let s = server(&fake);
    let e = error_text(&s.get_contact(params(json!({"id": "nope"}))).await.unwrap());
    assert!(e.starts_with("not_found:"), "{e}");
    let e = error_text(
        &s.get_contact(params(json!({"id": GROUP_UID})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
}

#[tokio::test]
async fn an_unparseable_card_is_an_upstream_error() {
    let mut state = FakeState::icloud_like();
    state.put(
        "BROKEN.vcf",
        "BEGIN:VCARD\nVERSION:3.0\nUID:BROKEN-1\nFN:Broken\n",
    );
    let fake = FakeCardDav::start(state).await;
    let s = server(&fake);
    let e = error_text(
        &s.get_contact(params(json!({"id": "BROKEN-1"})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("upstream:"), "{e}");
    let v = json(&s.search_contacts(params(json!({}))).await.unwrap());
    assert_eq!(v["total"], 0);
}

#[tokio::test]
async fn get_my_contact_returns_my_card_or_says_none_is_set() {
    let fake = seeded().await;
    let v = json(&server(&fake).get_my_contact().await.unwrap());
    assert_eq!(v["id"], PERSON_UID);
    assert_eq!(v["is_me"], true);
    fake.state().me = None;
    let fresh = server(&fake);
    let e = error_text(&fresh.get_my_contact().await.unwrap());
    assert_eq!(
        e,
        "not_found: not found: no My Card is set in iCloud Contacts"
    );
}

#[tokio::test]
async fn list_groups_counts_known_members() {
    let fake = seeded().await;
    let v = json(&server(&fake).list_groups().await.unwrap());
    assert_eq!(
        v,
        json!([{"id": GROUP_UID, "name": "Book Club", "member_count": 2}])
    );
}

#[tokio::test]
async fn the_server_names_itself_and_sends_its_icon() {
    let fake = seeded().await;
    let info = server(&fake).get_info();
    assert_eq!(info.server_info.name, "mcp-icloud-contacts");
    let icons = info.server_info.icons.expect("icons");
    assert_eq!(icons.len(), 2);
    assert!(icons[0].src.starts_with("data:image/png;base64,"));
    assert!(info.instructions.unwrap().contains("update_contact"));
}
