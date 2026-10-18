mod support;
use serde_json::{Value, json};
use support::cards::{COMPANY_UID, GROUP_UID, PERSON_UID, contact, seed};
use support::fake_carddav::{FakeCardDav, FakeState};
use support::tools::{json, server};

const DUP_UID: &str = "D0D0D0D0-0000-4000-8000-000000000001";
const DUP_FILE: &str = "D0D0D0D0-0000-4000-8000-000000000001.vcf";
const SAM_UID: &str = "D0D0D0D0-0000-4000-8000-000000000002";
const SAM_FILE: &str = "D0D0D0D0-0000-4000-8000-000000000002.vcf";
const SAM2_UID: &str = "D0D0D0D0-0000-4000-8000-000000000003";
const SAM2_FILE: &str = "D0D0D0D0-0000-4000-8000-000000000003.vcf";

/// The usual account plus a second, thinner card for Alex (same work email, phone in national
/// form) and two cards for Sam Example that share only the name.
fn state() -> FakeState {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state.put(
        DUP_FILE,
        &contact(
            DUP_UID,
            "Alex",
            "Example",
            &[
                "EMAIL;type=INTERNET;type=WORK:Alex.Work@example.org",
                "TEL;type=CELL:07700 900123",
            ],
        ),
    );
    state.put(SAM_FILE, &contact(SAM_UID, "Sam", "Example", &[]));
    state.put(
        SAM2_FILE,
        &contact(SAM2_UID, "Sam", "Example", &["NOTE:the second Sam"]),
    );
    state
}

#[tokio::test]
async fn find_duplicate_contacts_groups_contacts_with_reasons() {
    let fake = FakeCardDav::start(state()).await;
    let v = json(&server(&fake).find_duplicate_contacts().await.unwrap());
    let groups = v["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{v}");
    let alex = &groups[0];
    assert_eq!(alex["reasons"], json!(["email", "phone"]));
    let ids: Vec<&Value> = alex["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["id"])
        .collect();
    assert_eq!(
        ids,
        [PERSON_UID, DUP_UID],
        "the card with a photo comes first"
    );
    assert_eq!(alex["contacts"][0]["has_photo"], true);
    assert_eq!(alex["contacts"][0]["is_me"], true);
    assert_eq!(alex["contacts"][0]["updated"], "2026-10-17T18:23:20Z");
    assert_eq!(alex["contacts"][1]["has_photo"], false);
    let sam = &groups[1];
    assert_eq!(sam["reasons"], json!(["name"]));
    let ids: Vec<&Value> = sam["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["id"])
        .collect();
    assert_eq!(
        ids,
        [SAM2_UID, SAM_UID],
        "the card with more details comes first"
    );
    let all = v.to_string();
    assert!(
        !all.contains(COMPANY_UID) && !all.contains(GROUP_UID),
        "{all}"
    );
}

#[tokio::test]
async fn unparseable_cards_are_skipped() {
    let mut state = state();
    state.put(
        "BROKEN.vcf",
        "BEGIN:VCARD\nVERSION:3.0\nUID:BROKEN-1\nFN:Sam Example\nbad line\nEND:VCARD\n",
    );
    let fake = FakeCardDav::start(state).await;
    let v = json(&server(&fake).find_duplicate_contacts().await.unwrap());
    assert_eq!(v["groups"].as_array().unwrap().len(), 2, "{v}");
    assert!(!v.to_string().contains("BROKEN-1"), "{v}");
}

#[tokio::test]
async fn no_duplicates_gives_no_groups() {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    let fake = FakeCardDav::start(state).await;
    let v = json(&server(&fake).find_duplicate_contacts().await.unwrap());
    assert_eq!(v, json!({"groups": []}));
}
