mod support;
use serde_json::{Value, json};
use support::cards::{
    COMPANY_UID, GROUP_FILE, GROUP_UID, PERSON, PERSON_FILE, PERSON_UID, contact, group, seed,
};
use support::fake_carddav::{BOOK, FakeCardDav, FakeState};
use support::tools::{error_text, json, params, server};

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

const PHOTO_UID: &str = "D0D0D0D0-0000-4000-8000-000000000004";
const PHOTO_FILE: &str = "D0D0D0D0-0000-4000-8000-000000000004.vcf";
const OLD_GROUP_UID: &str = "D0D0D0D0-0000-4000-8000-0000000000A1";
const OLD_GROUP_FILE: &str = "D0D0D0D0-0000-4000-8000-0000000000A1.vcf";

/// Physical lines, for CRLF (stored cards) and LF (fixture files) alike.
fn lines(body: &str) -> Vec<String> {
    body.replace("\r\n", "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// `state()` plus a richer duplicate of Alex in a group of its own.
fn merge_state() -> FakeState {
    let mut state = state();
    state.put(
        DUP_FILE,
        &contact(
            DUP_UID,
            "Alex",
            "Example",
            &[
                "EMAIL;type=INTERNET;type=WORK:Alex.Work@example.org",
                "TEL;type=CELL:07700 900123",
                "TEL;type=WORK;type=VOICE:+1 202-555-0177",
                "NOTE:From the old phone",
            ],
        ),
    );
    state.put(
        OLD_GROUP_FILE,
        &group(OLD_GROUP_UID, "Old Friends", &[DUP_UID]),
    );
    state
}

#[tokio::test]
async fn merge_contacts_adds_what_is_missing_moves_groups_and_deletes() {
    let fake = FakeCardDav::start(merge_state()).await;
    let v = json(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": PERSON_UID, "merge_ids": [DUP_UID]}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(v["id"], PERSON_UID);
    assert_eq!(v["merged"], json!([DUP_UID]));
    assert_eq!(v["groups_updated"], json!([OLD_GROUP_UID]));
    assert_eq!(v["groups_not_updated"], json!([]));
    assert_eq!(v["not_deleted"], json!([]));
    assert_eq!(v["photo_not_kept"], json!([]));
    let phones: Vec<&Value> = v["contact"]["phones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["value"])
        .collect();
    assert_eq!(
        phones,
        [
            "+44 7700 900123",
            "07700 900456",
            "+1 202-555-0123",
            "+1 202-555-0177"
        ]
    );
    assert_eq!(
        v["contact"]["note"],
        "Met at the conference, 2019.\nLikes tea.\n\nFrom the old phone"
    );
    assert_eq!(v["contact"]["has_photo"], true);

    let state = fake.state();
    let body = state.card(PERSON_FILE).unwrap().body;
    let written = lines(&body);
    for old in lines(PERSON) {
        if old.starts_with("REV:") || old.starts_with("NOTE:") || old.is_empty() {
            continue;
        }
        assert!(written.contains(&old), "{old:?} changed in:\n{body}");
    }
    assert!(
        written.contains(&"TEL;type=WORK;type=VOICE:+1 202-555-0177".to_string()),
        "{body}"
    );
    assert!(state.card(DUP_FILE).is_none(), "the merged card is deleted");
    let old_group = state.card(OLD_GROUP_FILE).unwrap().body;
    assert!(
        old_group.contains(&format!("urn:uuid:{PERSON_UID}")),
        "{old_group}"
    );
    assert!(!old_group.contains(DUP_UID), "{old_group}");
    let put_paths: Vec<&str> = state
        .log
        .iter()
        .filter(|l| l.method == "PUT")
        .map(|l| l.path.as_str())
        .collect();
    assert_eq!(
        put_paths,
        [
            format!("{BOOK}{PERSON_FILE}"),
            format!("{BOOK}{OLD_GROUP_FILE}")
        ],
        "the kept card first, then the group; the seeded group already lists Alex"
    );
    assert!(state.card(GROUP_FILE).unwrap().body.contains(PERSON_UID));
}

#[tokio::test]
async fn merging_into_a_card_without_a_photo_reports_the_dropped_photo() {
    let mut state = merge_state();
    state.put(
        PHOTO_FILE,
        &contact(
            PHOTO_UID,
            "Alex",
            "Example",
            &[
                "EMAIL;type=INTERNET:Alex.Work@example.org",
                "PHOTO;VALUE=uri:https://gateway.icloud.com/contacts/1234567890/ck/card/0123456789abcdef0123456789abcdef",
            ],
        ),
    );
    let fake = FakeCardDav::start(state).await;
    let v = json(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": DUP_UID, "merge_ids": [PHOTO_UID]}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(v["photo_not_kept"], json!([PHOTO_UID]));
    assert_eq!(v["contact"]["has_photo"], false);
    assert!(fake.state().card(PHOTO_FILE).is_none());
}

#[tokio::test]
async fn nothing_new_writes_only_the_deletes() {
    let fake = FakeCardDav::start(state()).await;
    let v = json(
        &server(&fake)
            .merge_contacts(params(json!({"keep_id": SAM2_UID, "merge_ids": [SAM_UID]})))
            .await
            .unwrap(),
    );
    assert_eq!(v["merged"], json!([SAM_UID]));
    let state = fake.state();
    assert_eq!(state.count("PUT"), 0);
    assert_eq!(state.count("DELETE"), 1);
    assert!(state.card(SAM_FILE).is_none());
}

#[tokio::test]
async fn bad_merges_are_refused_before_any_write() {
    let fake = FakeCardDav::start(merge_state()).await;
    let s = server(&fake);
    let many: Vec<String> = (0..11).map(|i| format!("ID-{i}")).collect();
    for (args, want) in [
        (
            json!({"keep_id": DUP_UID, "merge_ids": []}),
            "validation: merge_ids is empty",
        ),
        (
            json!({"keep_id": DUP_UID, "merge_ids": many}),
            "validation: merge_ids has 11 ids; at most 10 can be merged at once",
        ),
        (
            json!({"keep_id": DUP_UID, "merge_ids": [format!(" {DUP_UID} ")]}),
            "validation: merge_ids contains keep_id",
        ),
        (
            json!({"keep_id": DUP_UID, "merge_ids": [SAM_UID, SAM_UID]}),
            "validation: merge_ids lists",
        ),
        (
            json!({"keep_id": DUP_UID, "merge_ids": [PERSON_UID]}),
            "validation: 5F1C2D3E-4A5B-4C6D-8E7F-901234567890 is the owner's own card",
        ),
        (
            json!({"keep_id": DUP_UID, "merge_ids": [GROUP_UID]}),
            "validation:",
        ),
        (
            json!({"keep_id": GROUP_UID, "merge_ids": [DUP_UID]}),
            "validation:",
        ),
        (
            json!({"keep_id": DUP_UID, "merge_ids": ["NOPE"]}),
            "not_found:",
        ),
    ] {
        let e = error_text(&s.merge_contacts(params(args)).await.unwrap());
        assert!(e.starts_with(want), "{e}");
    }
    let state = fake.state();
    assert_eq!(state.count("PUT"), 0);
    assert_eq!(state.count("DELETE"), 0);
}

#[tokio::test]
async fn a_conflict_on_the_kept_card_changes_nothing() {
    let fake = FakeCardDav::start(merge_state()).await;
    fake.state().fail.push_back(("PUT", 412));
    let e = error_text(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": PERSON_UID, "merge_ids": [DUP_UID]}),
            ))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("conflict:"), "{e}");
    let state = fake.state();
    assert_eq!(state.count("PUT"), 1);
    assert_eq!(state.count("DELETE"), 0);
    assert!(state.card(DUP_FILE).is_some());
    assert!(state.card(OLD_GROUP_FILE).unwrap().body.contains(DUP_UID));
}

#[tokio::test]
async fn a_failed_delete_is_reported_after_the_merge() {
    let fake = FakeCardDav::start(merge_state()).await;
    fake.state().fail.push_back(("DELETE", 500));
    let v = json(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": PERSON_UID, "merge_ids": [DUP_UID]}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(v["groups_updated"], json!([OLD_GROUP_UID]));
    assert_eq!(v["not_deleted"][0]["id"], DUP_UID);
    let err = v["not_deleted"][0]["error"].as_str().unwrap();
    assert!(err.starts_with("upstream:"), "{err}");
    assert!(fake.state().card(DUP_FILE).is_some());
}

#[tokio::test]
async fn a_group_listing_both_contacts_keeps_the_kept_one_once() {
    let mut state = merge_state();
    state.put(
        OLD_GROUP_FILE,
        &group(OLD_GROUP_UID, "Old Friends", &[PERSON_UID, DUP_UID]),
    );
    let fake = FakeCardDav::start(state).await;
    json(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": PERSON_UID, "merge_ids": [DUP_UID]}),
            ))
            .await
            .unwrap(),
    );
    let body = fake.state().card(OLD_GROUP_FILE).unwrap().body;
    assert_eq!(body.matches(PERSON_UID).count(), 1, "{body}");
    assert!(!body.contains(DUP_UID), "{body}");
}

#[tokio::test]
async fn a_merged_entry_that_cannot_be_written_refuses_the_whole_merge() {
    let mut state = merge_state();
    state.put(
        DUP_FILE,
        &contact(
            DUP_UID,
            "Alex",
            "Example",
            &["EMAIL;type=INTERNET:alex at home"],
        ),
    );
    let fake = FakeCardDav::start(state).await;
    let e = error_text(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": PERSON_UID, "merge_ids": [DUP_UID]}),
            ))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation: emails["), "{e}");
    let state = fake.state();
    assert_eq!(state.count("PUT"), 0);
    assert_eq!(state.count("DELETE"), 0);
}

#[tokio::test]
async fn a_merge_too_big_for_icloud_changes_nothing() {
    let mut state = merge_state();
    state.max_resource_size = PERSON.len() as u64 + 10;
    let fake = FakeCardDav::start(state).await;
    let e = error_text(
        &server(&fake)
            .merge_contacts(params(
                json!({"keep_id": PERSON_UID, "merge_ids": [DUP_UID]}),
            ))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("validation: the card would be"), "{e}");
    let state = fake.state();
    assert_eq!(state.count("PUT"), 0);
    assert_eq!(state.count("DELETE"), 0);
}
