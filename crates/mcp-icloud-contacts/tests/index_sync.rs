mod support;
use mcp_icloud_contacts::index::Store;
use mcp_icloud_contacts::vcard::parse::parse_card;
use mcp_icloud_dav::client::Precondition;
use support::cards::{
    COMPANY, COMPANY_FILE, COMPANY_UID, GROUP_FILE, GROUP_UID, PERSON_FILE, PERSON_UID, contact,
    seed,
};
use support::fake_carddav::{BOOK, FakeCardDav, FakeState};

async fn start(state: FakeState) -> (FakeCardDav, Store) {
    let fake = FakeCardDav::start(state).await;
    let store = Store::new(fake.client());
    (fake, store)
}

fn seeded() -> FakeState {
    let mut state = FakeState::icloud_like();
    seed(&mut state);
    state
}

#[tokio::test]
async fn a_full_load_reads_every_card_once() {
    let (fake, store) = start(seeded()).await;
    let idx = store.open().await.unwrap();
    assert_eq!(idx.len(), 3);
    let mut names: Vec<String> = idx.contacts().into_iter().map(|c| c.display_name).collect();
    names.sort();
    assert_eq!(names, ["Dr. Alex J. Example Jr.", "Example Plumbing Co"]);
    assert_eq!(idx.groups().len(), 1);
    let me = idx.me().expect("my card");
    assert_eq!(me.uid.as_deref(), Some(PERSON_UID));
    assert!(idx.contact(PERSON_UID).unwrap().is_me);
    assert!(!idx.contact(COMPANY_UID).unwrap().is_me);
    let s = fake.state();
    assert_eq!(s.count("PROPFIND"), 3);
    assert_eq!(s.reports("sync-collection").len(), 1);
    let gets = s.reports("addressbook-multiget");
    assert_eq!(gets.len(), 1);
    for file in [PERSON_FILE, COMPANY_FILE, GROUP_FILE] {
        assert!(gets[0].body.contains(file), "{file}");
    }
    assert!(
        !gets[0]
            .body
            .contains(&format!("{}<", BOOK.trim_end_matches('/')))
    );
}

#[tokio::test]
async fn later_calls_fetch_only_what_changed() {
    let (fake, store) = start(seeded()).await;
    drop(store.open().await.unwrap());
    {
        let mut s = fake.state();
        s.put(
            COMPANY_FILE,
            &COMPANY.replace("Example Plumbing Co", "Example Pipes Co"),
        );
        s.put("NEW.vcf", &contact("NEW-1", "Nia", "Example", &[]));
        s.remove(GROUP_FILE);
        s.log.clear();
    }
    let idx = store.open().await.unwrap();
    let mut names: Vec<String> = idx.contacts().into_iter().map(|c| c.display_name).collect();
    names.sort();
    assert_eq!(
        names,
        ["Dr. Alex J. Example Jr.", "Example Pipes Co", "Nia Example"]
    );
    assert!(idx.groups().is_empty());
    assert!(idx.card(GROUP_UID).is_err());
    drop(idx);
    {
        let s = fake.state();
        assert_eq!(s.count("PROPFIND"), 0);
        assert_eq!(s.reports("sync-collection").len(), 1);
        let gets = s.reports("addressbook-multiget");
        assert_eq!(gets.len(), 1);
        assert!(gets[0].body.contains(COMPANY_FILE));
        assert!(gets[0].body.contains("NEW.vcf"));
        assert!(!gets[0].body.contains(PERSON_FILE));
    }
    fake.state().log.clear();
    drop(store.open().await.unwrap());
    let s = fake.state();
    assert_eq!(s.reports("sync-collection").len(), 1);
    assert!(s.reports("addressbook-multiget").is_empty());
}

#[tokio::test]
async fn an_invalid_token_triggers_a_full_load() {
    let (fake, store) = start(seeded()).await;
    drop(store.open().await.unwrap());
    {
        let mut s = fake.state();
        s.expire_tokens();
        s.log.clear();
    }
    let idx = store.open().await.unwrap();
    assert_eq!(idx.len(), 3);
    drop(idx);
    let s = fake.state();
    assert_eq!(s.count("PROPFIND"), 3);
    assert_eq!(s.reports("sync-collection").len(), 2);
    assert_eq!(s.reports("addressbook-multiget").len(), 1);
}

#[tokio::test]
async fn our_own_writes_are_not_fetched_again() {
    let (fake, store) = start(seeded()).await;
    let mut idx = store.open().await.unwrap();
    let card = parse_card(&contact("NEW-2", "Noor", "Example", &[])).unwrap();
    let href = idx.book().unwrap().new_card_path("NEW-2");
    idx.put(store.client(), &href, card, Precondition::IfNoneMatch)
        .await
        .unwrap();
    assert_eq!(idx.contact("NEW-2").unwrap().display_name, "Noor Example");
    drop(idx);
    assert!(fake.state().card("NEW-2.vcf").is_some());
    fake.state().log.clear();
    let idx = store.open().await.unwrap();
    assert_eq!(idx.contacts().len(), 3);
    drop(idx);
    let s = fake.state();
    assert_eq!(s.reports("sync-collection").len(), 1);
    assert!(s.reports("addressbook-multiget").is_empty());
}

#[tokio::test]
async fn unparseable_cards_are_kept_but_not_listed() {
    let mut state = FakeState::icloud_like();
    state.put(
        "BROKEN.vcf",
        "BEGIN:VCARD\nVERSION:3.0\nUID:BROKEN-1\nFN:Broken\n",
    );
    let (_fake, store) = start(state).await;
    let idx = store.open().await.unwrap();
    assert_eq!(idx.len(), 1);
    assert!(idx.contacts().is_empty());
    assert!(idx.card("BROKEN-1").unwrap().card.is_none());
    assert_eq!(idx.contact("BROKEN-1").unwrap_err().kind(), "upstream");
}

#[tokio::test]
async fn many_cards_are_fetched_in_batches_of_100() {
    let mut state = FakeState::icloud_like();
    for i in 0..250 {
        state.put(
            &format!("C{i:03}.vcf"),
            &contact(&format!("C{i:03}"), "Person", &format!("{i:03}"), &[]),
        );
    }
    let (fake, store) = start(state).await;
    assert_eq!(store.open().await.unwrap().contacts().len(), 250);
    assert_eq!(fake.state().reports("addressbook-multiget").len(), 3);
}

#[tokio::test]
async fn group_views_ignore_members_that_no_longer_exist() {
    let (_fake, store) = start(seeded()).await;
    let idx = store.open().await.unwrap();
    let group = idx.group(GROUP_UID).unwrap();
    assert_eq!(group.name, "Book Club");
    assert_eq!(group.members.len(), 3);
    let mut known: Vec<String> = idx
        .known_members(&group)
        .into_iter()
        .map(|c| c.id)
        .collect();
    known.sort();
    assert_eq!(known, [COMPANY_UID, PERSON_UID]);
    let alex = idx.contact(PERSON_UID).unwrap();
    assert_eq!(alex.groups.len(), 1);
    assert_eq!(alex.groups[0].name, "Book Club");
    assert_eq!(idx.groups_with(PERSON_UID).len(), 1);
}

#[tokio::test]
async fn ids_of_the_wrong_kind_are_refused() {
    let (_fake, store) = start(seeded()).await;
    let idx = store.open().await.unwrap();
    assert_eq!(
        idx.contact_card(GROUP_UID).unwrap_err().kind(),
        "validation"
    );
    assert_eq!(idx.group_card(PERSON_UID).unwrap_err().kind(), "validation");
    assert_eq!(idx.card("no-such-id").unwrap_err().kind(), "not_found");
}

#[tokio::test]
async fn a_failed_refresh_is_returned_to_the_caller() {
    let (fake, store) = start(seeded()).await;
    drop(store.open().await.unwrap());
    fake.state().fail.extend([("REPORT", 500), ("REPORT", 500)]);
    let e = store.open().await.unwrap_err();
    assert_eq!(e.kind(), "upstream");
    assert_eq!(store.open().await.unwrap().len(), 3);
}
