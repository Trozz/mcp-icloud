mod support;
use serde_json::json;
use support::cards::{
    COMPANY_FILE, COMPANY_UID, GROUP_FILE, GROUP_UID, PERSON_FILE, PERSON_UID, contact, seed,
};
use support::fake_carddav::{BOOK, FakeCardDav, FakeState};
use support::tools::{error_text, json, params, server};

async fn seeded() -> FakeCardDav {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state.put("NEW.vcf", &contact("NEW-1", "Nia", "Example", &[]));
    FakeCardDav::start(state).await
}

#[tokio::test]
async fn create_group_writes_an_apple_group_card() {
    let fake = seeded().await;
    let v = json(
        &server(&fake)
            .create_group(params(json!({
                "name": "Neighbours",
                "member_ids": [COMPANY_UID, "NEW-1", COMPANY_UID]
            })))
            .await
            .unwrap(),
    );
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["name"], "Neighbours");
    assert_eq!(v["member_count"], 2);
    assert_eq!(
        v["members"],
        json!([
            {"id": COMPANY_UID, "display_name": "Example Plumbing Co"},
            {"id": "NEW-1", "display_name": "Nia Example"}
        ])
    );
    let state = fake.state();
    let put = state.log.iter().find(|l| l.method == "PUT").unwrap();
    assert_eq!(put.path, format!("{BOOK}{id}.vcf"));
    assert_eq!(put.if_none_match.as_deref(), Some("*"));
    let body = state.card(&format!("{id}.vcf")).unwrap().body;
    for want in [
        "\r\nN:Neighbours\r\nFN:Neighbours\r\nX-ADDRESSBOOKSERVER-KIND:group\r\n",
        &format!(
            "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:{COMPANY_UID}\r\nX-ADDRESSBOOKSERVER-MEMBER:urn:uuid:NEW-1\r\n"
        ),
    ] {
        assert!(body.contains(want), "{want} missing from:\n{body}");
    }
}

#[tokio::test]
async fn create_group_refuses_bad_members_and_names() {
    let fake = seeded().await;
    let s = server(&fake);
    for (args, want) in [
        (json!({"name": " "}), "validation: name is empty"),
        (
            json!({"name": "G", "member_ids": ["nope"]}),
            "validation: member_ids contains nope, which is not a contact id",
        ),
        (
            json!({"name": "G", "member_ids": [GROUP_UID]}),
            "validation: member_ids contains",
        ),
    ] {
        let e = error_text(&s.create_group(params(args)).await.unwrap());
        assert!(e.starts_with(want), "{e}");
    }
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn update_group_renames_adds_and_removes_members() {
    let fake = seeded().await;
    let before = fake.state().card(GROUP_FILE).unwrap();
    let v = json(
        &server(&fake)
            .update_group(params(json!({
                "id": GROUP_UID,
                "name": "Reading Group",
                "add_ids": ["NEW-1", PERSON_UID],
                "remove_ids": [COMPANY_UID, "DEADBEEF-0000-4000-8000-000000000000"]
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["name"], "Reading Group");
    assert_eq!(
        v["members"],
        json!([
            {"id": PERSON_UID, "display_name": "Dr. Alex J. Example Jr."},
            {"id": "NEW-1", "display_name": "Nia Example"}
        ])
    );
    let state = fake.state();
    let put = state.log.iter().find(|l| l.method == "PUT").unwrap();
    assert_eq!(put.if_match.as_deref(), Some(before.etag.as_str()));
    let body = state.card(GROUP_FILE).unwrap().body;
    assert!(
        body.contains("\r\nN:Reading Group\r\nFN:Reading Group\r\n"),
        "{body}"
    );
    assert!(!body.contains(COMPANY_UID), "{body}");
    assert!(!body.contains("DEADBEEF"), "{body}");
    assert_eq!(body.matches(PERSON_UID).count(), 1, "{body}");
    assert!(
        state.card(COMPANY_FILE).is_some(),
        "removing a member keeps the contact"
    );
}

#[tokio::test]
async fn update_group_accepts_urn_uuid_remove_ids() {
    let fake = seeded().await;
    server(&fake)
        .update_group(params(json!({
            "id": GROUP_UID,
            "remove_ids": [format!("urn:uuid:{COMPANY_UID}")]
        })))
        .await
        .unwrap();
    let body = fake.state().card(GROUP_FILE).unwrap().body;
    assert!(!body.contains(COMPANY_UID), "{body}");
    assert!(body.contains(PERSON_UID), "{body}");
}

#[tokio::test]
async fn update_group_refuses_bad_requests_before_writing() {
    let fake = seeded().await;
    let s = server(&fake);
    for (args, want) in [
        (json!({"id": GROUP_UID}), "validation: nothing to change"),
        (json!({"id": PERSON_UID, "name": "x"}), "validation:"),
        (
            json!({"id": GROUP_UID, "name": ""}),
            "validation: name is empty",
        ),
        (
            json!({"id": GROUP_UID, "add_ids": ["nope"]}),
            "validation: add_ids contains nope",
        ),
        (
            json!({"id": GROUP_UID, "remove_ids": ["NEW-1"]}),
            "validation: remove_ids contains NEW-1, which the group does not list",
        ),
    ] {
        let e = error_text(&s.update_group(params(args)).await.unwrap());
        assert!(e.starts_with(want), "{e}");
    }
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn an_update_group_that_changes_nothing_does_not_write() {
    let fake = seeded().await;
    let s = server(&fake);
    for args in [
        json!({"id": GROUP_UID, "add_ids": [], "remove_ids": []}),
        json!({"id": GROUP_UID, "name": " Book Club "}),
        json!({"id": GROUP_UID, "add_ids": [PERSON_UID]}),
    ] {
        let v = json(&s.update_group(params(args.clone())).await.unwrap());
        assert_eq!(v["name"], "Book Club", "{args}");
    }
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn a_conflicting_group_update_is_reported() {
    let fake = seeded().await;
    fake.state().fail.push_back(("PUT", 412));
    let e = error_text(
        &server(&fake)
            .update_group(params(json!({"id": GROUP_UID, "name": "Other"})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("conflict:"), "{e}");
}

#[tokio::test]
async fn delete_group_keeps_its_members() {
    let fake = seeded().await;
    let s = server(&fake);
    let v = json(
        &s.delete_group(params(json!({"id": GROUP_UID})))
            .await
            .unwrap(),
    );
    assert_eq!(v, json!({"deleted": true, "id": GROUP_UID}));
    {
        let state = fake.state();
        assert!(state.card(GROUP_FILE).is_none());
        assert!(state.card(PERSON_FILE).is_some());
        assert!(state.card(COMPANY_FILE).is_some());
    }
    assert_eq!(json(&s.list_groups().await.unwrap()), json!([]));
    let e = error_text(
        &s.delete_group(params(json!({"id": PERSON_UID})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
    assert!(fake.state().card(PERSON_FILE).is_some());
}
