mod support;
use serde_json::json;
use support::cards::{
    COMPANY_FILE, COMPANY_UID, GROUP_FILE, GROUP_UID, PERSON, PERSON_FILE, PERSON_UID, seed,
};
use support::fake_carddav::{BOOK, FakeCardDav, FakeState};
use support::tools::{error_text, json, params, server};

async fn seeded() -> FakeCardDav {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    FakeCardDav::start(state).await
}

fn lines(body: &str) -> Vec<String> {
    body.split("\r\n").map(str::to_string).collect()
}

#[tokio::test]
async fn create_contact_writes_a_new_card_with_if_none_match() {
    let fake = seeded().await;
    let s = server(&fake);
    let v = json(
        &s.create_contact(params(json!({
            "given_name": "Casey",
            "family_name": "Example",
            "emails": [{"value": "casey@example.com", "label": "work", "preferred": true}],
            "phones": [{"value": "+44 7700 900222", "label": "mobile"}],
            "related": [{"name": "Pat Example", "label": "best friend"}]
        })))
        .await
        .unwrap(),
    );
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 36);
    assert_eq!(id, id.to_uppercase());
    assert_eq!(v["display_name"], "Casey Example");
    assert_eq!(v["is_me"], false);
    assert_eq!(
        v["emails"],
        json!([{"value": "casey@example.com", "label": "work", "preferred": true}])
    );
    assert_eq!(
        v["related"],
        json!([{"name": "Pat Example", "label": "best friend"}])
    );
    let state = fake.state();
    let put = state.log.iter().find(|l| l.method == "PUT").unwrap();
    assert_eq!(put.path, format!("{BOOK}{id}.vcf"));
    assert_eq!(put.if_none_match.as_deref(), Some("*"));
    let body = state.card(&format!("{id}.vcf")).unwrap().body;
    let l = lines(&body);
    for want in [
        "PRODID:-//Trozz//mcp-icloud//EN",
        "N:Example;Casey;;;",
        "FN:Casey Example",
        "EMAIL;type=INTERNET;type=WORK;type=pref:casey@example.com",
        "TEL;type=CELL;type=VOICE:+44 7700 900222",
        "item1.X-ABRELATEDNAMES:Pat Example",
        "item1.X-ABLabel:best friend",
    ] {
        assert!(
            l.contains(&want.to_string()),
            "{want} missing from:\n{body}"
        );
    }
    assert!(l.contains(&format!("UID:{id}")));
}

#[tokio::test]
async fn create_contact_needs_a_name_and_writes_nothing_without_one() {
    let fake = seeded().await;
    let e = error_text(
        &server(&fake)
            .create_contact(params(json!({"note": "no name"})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation:"), "{e}");
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn update_contact_rewrites_only_the_lines_it_changes() {
    let fake = seeded().await;
    let before = fake.state().card(PERSON_FILE).unwrap();
    let v = json(
        &server(&fake)
            .update_contact(params(json!({
                "id": PERSON_UID,
                "nickname": "Al",
                "phones": [
                    {"value": "+44 7700 900123", "label": "mobile", "preferred": true},
                    {"value": "07700 900456", "label": "home"},
                    {"value": "07700 900999", "label": "summer house"}
                ]
            })))
            .await
            .unwrap(),
    );
    assert_eq!(v["nickname"], "Al");
    assert_eq!(
        v["phones"][2],
        json!({"value": "07700 900999", "label": "summer house"})
    );
    let state = fake.state();
    let put = state.log.iter().find(|l| l.method == "PUT").unwrap();
    assert_eq!(put.path, format!("{BOOK}{PERSON_FILE}"));
    assert_eq!(put.if_match.as_deref(), Some(before.etag.as_str()));
    let after = state.card(PERSON_FILE).unwrap().body;
    let (old, new) = (lines(&before.body), lines(&after));
    let removed: Vec<&String> = old.iter().filter(|l| !new.contains(l)).collect();
    let added: Vec<&String> = new.iter().filter(|l| !old.contains(l)).collect();
    assert_eq!(
        removed,
        [
            "NICKNAME:Lex",
            "item3.TEL:+1 202-555-0123",
            "item3.X-ABLabel:boat",
            "REV:2026-10-17T18:23:20Z"
        ]
    );
    assert_eq!(added.len(), 4, "{added:?}");
    assert_eq!(added[0], "NICKNAME:Al");
    assert_eq!(added[1], "item9.TEL:07700 900999");
    assert_eq!(added[2], "item9.X-ABLabel:summer house");
    assert!(added[3].starts_with("REV:"), "{added:?}");
}

#[tokio::test]
async fn a_photo_and_unknown_lines_survive_an_edit() {
    let fake = seeded().await;
    json(
        &server(&fake)
            .update_contact(params(json!({"id": PERSON_UID, "note": ""})))
            .await
            .unwrap(),
    );
    let after = fake.state().card(PERSON_FILE).unwrap().body;
    let original = PERSON.replace('\n', "\r\n");
    for kept in [
        "PHOTO;TYPE=JPEG;X-ABCROP-RECTANGLE=ABClipRect_1&0&359&2151&2151&AAECAwQFBgc\r\n ICQoLDA0ODw==;VALUE=uri:https://gateway.icloud.com/contacts/1234567890/ck/c\r\n ard/0123456789abcdef0123456789abcdef\r\n",
        "item1.X-ADDRESSING-GRAMMAR:AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIj\r\n",
        "X-IMAGEHASH:AAECAwQFBgcICQoLDA0ODw==\r\n",
    ] {
        assert!(original.contains(kept), "fixture changed: {kept}");
        assert!(after.contains(kept), "lost or rewritten: {kept}");
    }
    assert!(!after.contains("NOTE:"));
}

#[tokio::test]
async fn a_conflicting_update_is_reported_once_and_not_retried() {
    let fake = seeded().await;
    fake.state().fail.push_back(("PUT", 412));
    let e = error_text(
        &server(&fake)
            .update_contact(params(json!({"id": PERSON_UID, "nickname": "Al"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        e,
        "conflict: the contact changed since it was read; re-read it and retry"
    );
    assert_eq!(fake.state().count("PUT"), 1);
}

#[tokio::test]
async fn update_contact_refuses_bad_requests_before_writing() {
    let fake = seeded().await;
    let s = server(&fake);
    let cases = [
        (json!({"id": PERSON_UID}), "validation: nothing to change"),
        (json!({"id": GROUP_UID, "nickname": "x"}), "validation:"),
        (json!({"id": "nope", "nickname": "x"}), "not_found:"),
        (
            json!({"id": PERSON_UID, "emails": [{"value": "a@example.com", "preferred": true}, {"value": "b@example.com", "preferred": true}]}),
            "validation: at most one of emails may be preferred",
        ),
        (
            json!({"id": PERSON_UID, "birthday": "May 12"}),
            "validation: birthday must be",
        ),
    ];
    for (args, want) in cases {
        let e = error_text(&s.update_contact(params(args)).await.unwrap());
        assert!(e.starts_with(want), "{e}");
    }
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn a_card_over_the_size_limit_is_refused_before_writing() {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state.max_resource_size = 2_000;
    let fake = FakeCardDav::start(state).await;
    let e = error_text(
        &server(&fake)
            .update_contact(params(json!({"id": PERSON_UID, "note": "x".repeat(500)})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation: the card would be"), "{e}");
    assert_eq!(fake.state().count("PUT"), 0);
}

#[tokio::test]
async fn delete_contact_deletes_and_cleans_its_groups() {
    let fake = seeded().await;
    let before = fake.state().card(COMPANY_FILE).unwrap();
    let v = json(
        &server(&fake)
            .delete_contact(params(json!({"id": COMPANY_UID})))
            .await
            .unwrap(),
    );
    assert_eq!(
        v,
        json!({"deleted": true, "id": COMPANY_UID, "groups_updated": [GROUP_UID], "groups_not_updated": []})
    );
    let state = fake.state();
    assert!(state.card(COMPANY_FILE).is_none());
    let delete = state.log.iter().find(|l| l.method == "DELETE").unwrap();
    assert_eq!(delete.if_match.as_deref(), Some(before.etag.as_str()));
    let group = state.card(GROUP_FILE).unwrap().body;
    assert!(!group.contains(COMPANY_UID), "{group}");
    assert!(group.contains(&format!("urn:uuid:{PERSON_UID}")));
    assert!(group.contains("urn:uuid:DEADBEEF-0000-4000-8000-000000000000"));
}

#[tokio::test]
async fn a_failed_group_update_is_reported_and_the_contact_stays_deleted() {
    let fake = seeded().await;
    fake.state().fail.push_back(("PUT", 412));
    let v = json(
        &server(&fake)
            .delete_contact(params(json!({"id": COMPANY_UID})))
            .await
            .unwrap(),
    );
    assert_eq!(v["deleted"], true);
    assert_eq!(v["groups_updated"], json!([]));
    assert_eq!(v["groups_not_updated"][0]["id"], GROUP_UID);
    assert!(
        v["groups_not_updated"][0]["error"]
            .as_str()
            .unwrap()
            .starts_with("conflict:")
    );
    assert!(fake.state().card(COMPANY_FILE).is_none());
}

#[tokio::test]
async fn my_own_card_cannot_be_deleted() {
    let fake = seeded().await;
    let e = error_text(
        &server(&fake)
            .delete_contact(params(json!({"id": PERSON_UID})))
            .await
            .unwrap(),
    );
    assert!(
        e.starts_with("validation: this is the owner's own card"),
        "{e}"
    );
    assert_eq!(fake.state().count("DELETE"), 0);
    assert!(fake.state().card(PERSON_FILE).is_some());
}

#[tokio::test]
async fn passing_get_contact_output_back_changes_only_the_edited_line() {
    let fake = seeded().await;
    let s = server(&fake);
    let before = fake.state().card(PERSON_FILE).unwrap().body;
    let mut shown = json(
        &s.get_contact(params(json!({"id": PERSON_UID})))
            .await
            .unwrap(),
    );
    let fields = shown.as_object_mut().unwrap();
    for read_only in ["is_me", "has_photo", "updated", "groups", "social", "im"] {
        fields.remove(read_only);
    }
    fields.insert("job_title".into(), json!("Chief Tester"));
    json(&s.update_contact(params(shown)).await.unwrap());
    let after = fake.state().card(PERSON_FILE).unwrap().body;
    let (old, new) = (lines(&before), lines(&after));
    let removed: Vec<&String> = old.iter().filter(|l| !new.contains(l)).collect();
    let added: Vec<&String> = new.iter().filter(|l| !old.contains(l)).collect();
    assert_eq!(
        removed,
        ["TITLE:Chief Tinkerer", "REV:2026-10-17T18:23:20Z"]
    );
    assert_eq!(added[0], "TITLE:Chief Tester");
    assert_eq!(added.len(), 2, "{added:?}");
}

#[tokio::test]
async fn special_characters_read_back_exactly_as_written() {
    let fake = seeded().await;
    let s = server(&fake);
    let created = json(
        &s.create_contact(params(json!({
            "given_name": "Ann; Marie",
            "family_name": "O'Example, Jr",
            "organization": "Example; Sons, Ltd",
            "department": "R&D \\ Labs",
            "addresses": [{"street": "1 Example Street\nFlat 2; Rear", "city": "Exampleton, North"}],
            "related": [{"name": "Bo, the; Cat", "label": "pet: cat"}],
            "note": "Line one\nline two; with, punctuation \\ and more"
        })))
        .await
        .unwrap(),
    );
    let id = created["id"].as_str().unwrap();
    let got = json(
        &server(&fake)
            .get_contact(params(json!({"id": id})))
            .await
            .unwrap(),
    );
    assert_eq!(got["given_name"], "Ann; Marie");
    assert_eq!(got["family_name"], "O'Example, Jr");
    assert_eq!(got["display_name"], "Ann; Marie O'Example, Jr");
    assert_eq!(got["organization"], "Example; Sons, Ltd");
    assert_eq!(got["department"], "R&D \\ Labs");
    assert_eq!(
        got["addresses"][0]["street"],
        "1 Example Street\nFlat 2; Rear"
    );
    assert_eq!(got["addresses"][0]["city"], "Exampleton, North");
    assert_eq!(
        got["related"][0],
        json!({"name": "Bo, the; Cat", "label": "pet: cat"})
    );
    assert_eq!(
        got["note"],
        "Line one\nline two; with, punctuation \\ and more"
    );
}

#[tokio::test]
async fn parallel_tool_calls_share_one_load_and_both_succeed() {
    let fake = seeded().await;
    let s = server(&fake);
    let (found, updated) = tokio::join!(
        s.search_contacts(params(json!({"query": "plumbing"}))),
        s.update_contact(params(json!({"id": PERSON_UID, "nickname": "Al"}))),
    );
    assert_eq!(json(&found.unwrap())["total"], 1);
    assert_eq!(json(&updated.unwrap())["nickname"], "Al");
    let state = fake.state();
    assert_eq!(state.count("PROPFIND"), 3, "one discovery");
    assert_eq!(
        state.reports("addressbook-multiget").len(),
        1,
        "one full load"
    );
}

#[tokio::test]
async fn deleting_a_contact_cleans_every_group_that_lists_it() {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state.put(
        "SECOND.vcf",
        &support::cards::group("SECOND-GROUP", "Plumbers", &[COMPANY_UID]),
    );
    let fake = FakeCardDav::start(state).await;
    let v = json(
        &server(&fake)
            .delete_contact(params(json!({"id": COMPANY_UID})))
            .await
            .unwrap(),
    );
    let mut updated: Vec<String> = v["groups_updated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap().to_string())
        .collect();
    updated.sort();
    assert_eq!(updated, [GROUP_UID, "SECOND-GROUP"]);
    let state = fake.state();
    assert!(!state.card("SECOND.vcf").unwrap().body.contains(COMPANY_UID));
    assert!(!state.card(GROUP_FILE).unwrap().body.contains(COMPANY_UID));
}
