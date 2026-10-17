mod support;
use mcp_icloud_contacts::book::{DEFAULT_MAX_RESOURCE_SIZE, discover};
use support::cards::{PERSON, PERSON_FILE};
use support::fake_carddav::{BOOK, FakeCardDav, FakeState, HOME};

#[tokio::test]
async fn finds_the_card_address_book_and_my_card() {
    let mut state = FakeState::icloud_like();
    state.put(PERSON_FILE, PERSON);
    state.me = Some(PERSON_FILE.into());
    let fake = FakeCardDav::start(state).await;
    let book = discover(&fake.client()).await.unwrap();
    assert_eq!(book.home.path(), HOME);
    assert_eq!(book.url.path(), BOOK);
    assert_eq!(book.url.origin(), fake.url.origin());
    assert_eq!(
        book.me_card.as_deref(),
        Some(format!("{BOOK}{PERSON_FILE}").as_str())
    );
    assert_eq!(book.max_resource_size, DEFAULT_MAX_RESOURCE_SIZE);
    assert_eq!(
        book.path(&format!("{BOOK}{PERSON_FILE}")).unwrap(),
        format!("{BOOK}{PERSON_FILE}")
    );
    assert_eq!(book.new_card_path("ABC"), format!("{BOOK}ABC.vcf"));
}

#[tokio::test]
async fn follows_a_redirect_at_the_entry_point() {
    let mut state = FakeState::icloud_like();
    state.redirect_root = true;
    let fake = FakeCardDav::start(state).await;
    let book = discover(&fake.client()).await.unwrap();
    assert_eq!(book.url.path(), BOOK);
    assert_eq!(fake.state().log[0].path, "/");
    assert_eq!(fake.state().log[1].path, "/root/");
}

#[tokio::test]
async fn prefers_the_card_collection_and_works_without_my_card() {
    let mut state = FakeState::icloud_like();
    state.extra_books = vec!["shared".into()];
    state.max_resource_size = 1000;
    let fake = FakeCardDav::start(state).await;
    let book = discover(&fake.client()).await.unwrap();
    assert_eq!(book.url.path(), BOOK);
    assert_eq!(book.me_card, None);
    assert_eq!(book.max_resource_size, 1000);
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let fake = FakeCardDav::start(FakeState::icloud_like()).await;
    let e = discover(&fake.client_with("wrong")).await.unwrap_err();
    assert_eq!(e.kind(), "auth");
}
