mod support;
use serde_json::json;
use support::cards::{COMPANY_FILE, COMPANY_UID, PERSON_FILE, PERSON_UID, contact, seed};
use support::fake_carddav::{FakeCardDav, FakeState};
use support::tools::{error_text, json, params, server, text_resource};

const LOWER_UID: &str = "E0E0E0E0-0000-4000-8000-000000000001";
const LOWER_FILE: &str = "E0E0E0E0-0000-4000-8000-000000000001.vcf";
const ODD_UID: &str = "ZZ probe/1 %";
const ODD_FILE: &str = "E0E0E0E0-0000-4000-8000-000000000002.vcf";

async fn seeded() -> FakeCardDav {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    FakeCardDav::start(state).await
}

/// The body iCloud (the fake) stores for a card file.
fn stored(fake: &FakeCardDav, file: &str) -> String {
    fake.state().card(file).expect("stored card").body
}

#[tokio::test]
async fn a_contact_exports_exactly_as_stored() {
    let fake = seeded().await;
    let s = server(&fake);
    for (id, file, name) in [
        (PERSON_UID, PERSON_FILE, "Dr. Alex J. Example Jr."),
        (COMPANY_UID, COMPANY_FILE, "Example Plumbing Co"),
    ] {
        let r = s.export_vcard(params(json!({"id": id}))).await.unwrap();
        let body = stored(&fake, file);
        let (uri, mime_type, text) = text_resource(&r);
        assert_eq!(text, body, "{id} is exported byte for byte");
        assert_eq!(
            json(&r),
            json!({
                "id": id,
                "kind": "contact",
                "display_name": name,
                "cards": 1,
                "missing_members": [],
                "bytes": body.len(),
            })
        );
        assert_eq!(uri, format!("icloud-contacts://export/{id}.vcf"));
        assert_eq!(mime_type.as_deref(), Some("text/vcard"));
    }
    let person = stored(&fake, PERSON_FILE);
    assert!(
        person
            .split("\r\n")
            .any(|l| l.len() == 76 && l.starts_with(' ')),
        "the person fixture must keep Apple's 76-octet continuation lines"
    );
    assert!(person.contains("&AAECAwQFBgc\r\n ICQoLDA0ODw==;VALUE=uri:"));
}

#[tokio::test]
async fn the_id_is_trimmed_like_every_other_tool() {
    let fake = seeded().await;
    let r = server(&fake)
        .export_vcard(params(json!({"id": format!("  {PERSON_UID} ")})))
        .await
        .unwrap();
    assert_eq!(json(&r)["id"], PERSON_UID);
    let (uri, _, _) = text_resource(&r);
    assert_eq!(uri, format!("icloud-contacts://export/{PERSON_UID}.vcf"));
}

#[tokio::test]
async fn an_export_after_an_edit_is_what_was_written() {
    let fake = seeded().await;
    let s = server(&fake);
    json(
        &s.update_contact(params(json!({"id": PERSON_UID, "nickname": "Al"})))
            .await
            .unwrap(),
    );
    let r = s
        .export_vcard(params(json!({"id": PERSON_UID})))
        .await
        .unwrap();
    let (_, _, text) = text_resource(&r);
    assert_eq!(text, stored(&fake, PERSON_FILE));
    assert!(text.contains("\r\nNICKNAME:Al\r\n"), "{}", text.len());
}

#[tokio::test]
async fn begin_and_end_lines_are_written_in_capitals() {
    let mut state = FakeState::icloud_like();
    let body = contact(
        LOWER_UID,
        "Robin",
        "Sample",
        &["NOTE:lower-case begin and end"],
    )
    .replace("BEGIN:VCARD", "begin:vcard")
    .replace("END:VCARD", "end:vcard");
    state.put(LOWER_FILE, &body);
    let fake = FakeCardDav::start(state).await;
    let r = server(&fake)
        .export_vcard(params(json!({"id": LOWER_UID})))
        .await
        .unwrap();
    let (_, _, text) = text_resource(&r);
    assert_eq!(
        text,
        body.replace("begin:vcard", "BEGIN:VCARD")
            .replace("end:vcard", "END:VCARD"),
        "only the BEGIN and END lines change"
    );
}

#[tokio::test]
async fn uri_reserved_characters_in_the_id_are_encoded() {
    let mut state = FakeState::icloud_like();
    state.put(ODD_FILE, &contact(ODD_UID, "Robin", "Sample", &[]));
    let fake = FakeCardDav::start(state).await;
    let r = server(&fake)
        .export_vcard(params(json!({"id": ODD_UID})))
        .await
        .unwrap();
    assert_eq!(json(&r)["id"], ODD_UID);
    let (uri, _, _) = text_resource(&r);
    assert_eq!(uri, "icloud-contacts://export/ZZ%20probe%2F1%20%25.vcf");
}

#[tokio::test]
async fn unknown_and_unparseable_ids_are_errors() {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state.put(
        "BROKEN.vcf",
        "BEGIN:VCARD\nVERSION:3.0\nUID:BROKEN-1\nFN:Broken\n",
    );
    let fake = FakeCardDav::start(state).await;
    let s = server(&fake);
    let e = error_text(&s.export_vcard(params(json!({"id": "nope"}))).await.unwrap());
    assert!(e.starts_with("not_found:"), "{e}");
    let e = error_text(
        &s.export_vcard(params(json!({"id": "BROKEN-1"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        e,
        "upstream: iCloud error: the card BROKEN-1 could not be parsed; it is left unchanged"
    );
}
