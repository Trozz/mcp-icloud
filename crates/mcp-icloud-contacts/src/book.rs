//! Finding the address book: principal, home, then the `card` collection, with `me-card` and the
//! largest card iCloud accepts.

use crate::error::{ContactsError, Result};
use mcp_icloud_dav::client::DavClient;
use mcp_icloud_dav::discovery::{HomeSet, discover_home, join};
use mcp_icloud_dav::xml::{CALSERVER, CARDDAV, DavResponse};
use url::Url;

/// iCloud's `max-resource-size`, used when the server does not say.
pub const DEFAULT_MAX_RESOURCE_SIZE: u64 = 563_200;

#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    /// The address book home, on the account's partition host.
    pub home: Url,
    /// The address book collection.
    pub url: Url,
    /// Path of the owner's own card ("My Card"), when one is set.
    pub me_card: Option<String>,
    /// Largest card iCloud accepts, in bytes.
    pub max_resource_size: u64,
}

impl Book {
    /// The URL of a card from its href (a path or an absolute URL).
    pub fn card_url(&self, href: &str) -> Result<Url> {
        Ok(join(&self.url, href)?)
    }

    /// The path of a card from its href, the form the index keys cards by.
    pub fn path(&self, href: &str) -> Result<String> {
        Ok(self.card_url(href)?.path().to_string())
    }

    /// The path of a new card named after its UID.
    pub fn new_card_path(&self, uid: &str) -> String {
        format!("{}{uid}.vcf", self.url.path())
    }
}

fn propfind_book() -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:card="{CARDDAV}" xmlns:cs="{CALSERVER}"><d:prop><d:resourcetype/><d:displayname/><d:sync-token/><card:max-resource-size/><cs:me-card/></d:prop></d:propfind>"#
    )
}

/// Runs discovery from the client's base URL. The address book is the collection whose
/// resourcetype includes `addressbook`, preferring one whose path ends in `/card/`.
pub async fn discover(client: &DavClient) -> Result<Book> {
    let home = discover_home(client, HomeSet::AddressBook).await?;
    let (home_url, listing) = client.propfind(&home.home, 1, propfind_book()).await?;
    let books: Vec<&DavResponse> = listing
        .iter()
        .filter(|r| r.has_type(CARDDAV, "addressbook"))
        .collect();
    let book = books
        .iter()
        .find(|r| r.href.ends_with("/card/"))
        .or(books.first())
        .ok_or_else(|| ContactsError::Upstream("the server listed no address book".into()))?;
    let me_card = listing
        .iter()
        .find_map(|r| r.props.me_card.clone())
        .map(|href| join(&home_url, &href).map(|u| u.path().to_string()))
        .transpose()?;
    let book = Book {
        url: join(&home_url, &book.href)?,
        home: home_url,
        me_card,
        max_resource_size: book
            .props
            .max_resource_size
            .unwrap_or(DEFAULT_MAX_RESOURCE_SIZE),
    };
    tracing::info!(
        my_card = book.me_card.is_some(),
        "contacts discovery complete"
    );
    Ok(book)
}
