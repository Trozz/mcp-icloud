//! The in-memory index of every card in the address book, brought up to date with
//! `sync-collection` at the start of every tool call.

use crate::book::{self, Book};
use crate::error::{ContactsError, Result};
use crate::vcard::model::{Contact, Group, GroupRef};
use crate::vcard::parse::{self, Kind};
use crate::vcard::patch::check_size;
use mcp_icloud_dav::DavError;
use mcp_icloud_dav::client::{DavClient, Precondition};
use mcp_icloud_dav::line::Component;
use mcp_icloud_dav::xml::{addressbook_multiget, sync_collection};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

/// Cards fetched per `addressbook-multiget`.
pub const MULTIGET_BATCH: usize = 100;

/// One card as stored in iCloud.
#[derive(Debug, Clone)]
pub struct Card {
    /// The card's path, the key of the index.
    pub href: String,
    pub etag: String,
    pub uid: Option<String>,
    pub kind: Kind,
    /// The line tree; None when the card could not be parsed.
    pub card: Option<Component>,
    /// The contact view of a parsed contact card.
    pub contact: Option<Contact>,
    /// The group view of a parsed group card.
    pub group: Option<Group>,
}

#[derive(Debug, Default)]
pub struct Index {
    book: Option<Book>,
    token: Option<String>,
    cards: BTreeMap<String, Card>,
    by_uid: HashMap<String, String>,
}

impl Index {
    pub fn book(&self) -> Result<&Book> {
        self.book
            .as_ref()
            .ok_or_else(|| ContactsError::Internal("the address book is not loaded".into()))
    }

    /// Brings the index up to date: a full load the first time, after a restart or when iCloud
    /// refuses the sync token; otherwise only the changes since the last call.
    pub async fn refresh(&mut self, client: &DavClient) -> Result<()> {
        let (Some(book), Some(token)) = (self.book.clone(), self.token.clone()) else {
            return self.full_load(client).await;
        };
        let changes = match client
            .sync_report(&book.url, sync_collection(Some(&token)))
            .await
        {
            Ok(c) => c,
            Err(DavError::Conflict) => {
                tracing::info!("contacts sync token no longer valid; reloading");
                return self.full_load(client).await;
            }
            Err(DavError::NotFound(_)) => return self.full_load(client).await,
            Err(e) => return Err(e.into()),
        };
        let mut fetch = Vec::new();
        let mut removed = 0;
        for r in &changes.responses {
            let Some(path) = card_path(&book, &r.href)? else {
                continue;
            };
            if r.status == Some(404) {
                self.remove(&path);
                removed += 1;
                continue;
            }
            let same = r
                .props
                .etag
                .as_ref()
                .is_some_and(|e| self.cards.get(&path).is_some_and(|c| &c.etag == e));
            if !same {
                fetch.push(path);
            }
        }
        let changed = fetch.len();
        self.fetch(client, &book, fetch).await?;
        if let Some(t) = changes.sync_token {
            self.token = Some(t);
        }
        tracing::debug!(changed, removed, "contacts refreshed");
        Ok(())
    }

    async fn full_load(&mut self, client: &DavClient) -> Result<()> {
        let book = book::discover(client).await?;
        let listing = client.sync_report(&book.url, sync_collection(None)).await?;
        let mut paths = Vec::new();
        for r in &listing.responses {
            if r.status == Some(404) {
                continue;
            }
            if let Some(path) = card_path(&book, &r.href)? {
                paths.push(path);
            }
        }
        self.cards.clear();
        self.by_uid.clear();
        self.book = Some(book.clone());
        self.token = None;
        self.fetch(client, &book, paths).await?;
        self.token = listing.sync_token;
        tracing::info!(cards = self.cards.len(), "contacts loaded");
        Ok(())
    }

    async fn fetch(&mut self, client: &DavClient, book: &Book, paths: Vec<String>) -> Result<()> {
        for chunk in paths.chunks(MULTIGET_BATCH) {
            let found = client
                .report(&book.url, addressbook_multiget(chunk))
                .await?;
            for r in found {
                let Some(path) = card_path(book, &r.href)? else {
                    continue;
                };
                match (r.status, r.props.etag, r.props.address_data) {
                    (Some(404), _, _) => self.remove(&path),
                    (_, Some(etag), Some(data)) => self.insert(path, etag, &data),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn insert(&mut self, href: String, etag: String, text: &str) {
        match parse::parse_card(text) {
            Ok(card) => self.put_parsed(href, etag, card),
            Err(e) => {
                tracing::warn!(card = %href, kind = e.kind(), "a card could not be parsed; it is left unchanged");
                self.remove(&href);
                let uid = parse::uid_of_text(text);
                if let Some(u) = &uid {
                    self.by_uid.insert(u.clone(), href.clone());
                }
                self.cards.insert(
                    href.clone(),
                    Card {
                        href,
                        etag,
                        uid,
                        kind: Kind::Contact,
                        card: None,
                        contact: None,
                        group: None,
                    },
                );
            }
        }
    }

    fn put_parsed(&mut self, href: String, etag: String, card: Component) {
        self.remove(&href);
        let uid = parse::uid(&card);
        let kind = parse::kind(&card);
        let id = uid.clone().unwrap_or_default();
        let (contact, group) = match kind {
            Kind::Contact => (Some(parse::parse_contact(&card, &id)), None),
            Kind::Group => (None, Some(parse::parse_group(&card, &id))),
        };
        if let Some(u) = &uid {
            self.by_uid.insert(u.clone(), href.clone());
        }
        self.cards.insert(
            href.clone(),
            Card {
                href,
                etag,
                uid,
                kind,
                card: Some(card),
                contact,
                group,
            },
        );
    }

    fn remove(&mut self, href: &str) {
        if let Some(old) = self.cards.remove(href)
            && let Some(uid) = old.uid
            && self.by_uid.get(&uid).map(String::as_str) == Some(href)
        {
            self.by_uid.remove(&uid);
        }
    }

    /// Number of cards (contacts, groups and unparseable cards).
    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// A card by its id (UID).
    pub fn card(&self, id: &str) -> Result<&Card> {
        self.by_uid
            .get(id.trim())
            .and_then(|h| self.cards.get(h))
            .ok_or_else(|| ContactsError::NotFound(format!("no contact or group has the id {id}")))
    }

    /// A contact card by id; a group id is a validation error, an unparseable card upstream.
    pub fn contact_card(&self, id: &str) -> Result<(&Card, &Component)> {
        let card = self.card(id)?;
        if card.kind == Kind::Group {
            return Err(ContactsError::invalid(format!(
                "{id} is a group; use the group tools"
            )));
        }
        match &card.card {
            Some(c) => Ok((card, c)),
            None => Err(ContactsError::Upstream(format!(
                "the card {id} could not be parsed; it is left unchanged"
            ))),
        }
    }

    /// A group card by id; a contact id is a validation error.
    pub fn group_card(&self, id: &str) -> Result<(&Card, &Component)> {
        let card = self.card(id)?;
        match (&card.card, card.kind) {
            (Some(c), Kind::Group) => Ok((card, c)),
            _ => Err(ContactsError::invalid(format!(
                "{id} is not a group; use list_groups for group ids"
            ))),
        }
    }

    pub fn is_me(&self, card: &Card) -> bool {
        self.book
            .as_ref()
            .and_then(|b| b.me_card.as_deref())
            .is_some_and(|m| m == card.href)
    }

    /// The owner's own card, when `me-card` names a known contact.
    pub fn me(&self) -> Option<&Card> {
        let me = self.book.as_ref()?.me_card.as_deref()?;
        self.cards.get(me).filter(|c| c.contact.is_some())
    }

    /// Every parsed contact with an id, with `is_me` set (groups are not filled in).
    pub fn contacts(&self) -> Vec<Contact> {
        self.cards
            .values()
            .filter(|c| c.uid.is_some())
            .filter_map(|c| {
                let mut view = c.contact.clone()?;
                view.is_me = self.is_me(c);
                Some(view)
            })
            .collect()
    }

    /// One contact with `is_me` and its groups.
    pub fn contact(&self, id: &str) -> Result<Contact> {
        let (card, _) = self.contact_card(id)?;
        let mut view = card
            .contact
            .clone()
            .ok_or_else(|| ContactsError::Internal("a parsed contact has no view".into()))?;
        view.is_me = self.is_me(card);
        let uid = card.uid.clone().unwrap_or_default();
        view.groups = self
            .groups()
            .into_iter()
            .filter(|g| g.members.contains(&uid))
            .map(|g| GroupRef {
                id: g.id,
                name: g.name,
            })
            .collect();
        Ok(view)
    }

    /// Every group, by name.
    pub fn groups(&self) -> Vec<Group> {
        let mut groups: Vec<Group> = self
            .cards
            .values()
            .filter(|c| c.uid.is_some())
            .filter_map(|c| c.group.clone())
            .collect();
        groups.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        groups
    }

    pub fn group(&self, id: &str) -> Result<Group> {
        let (card, _) = self.group_card(id)?;
        card.group
            .clone()
            .ok_or_else(|| ContactsError::Internal("a parsed group has no view".into()))
    }

    /// The members of a group that are known contacts. Member UIDs that match no contact (left
    /// behind by deletions) are ignored.
    pub fn known_members(&self, group: &Group) -> Vec<Contact> {
        group
            .members
            .iter()
            .filter_map(|uid| self.card(uid).ok())
            .filter(|c| c.kind == Kind::Contact)
            .filter_map(|c| c.contact.clone())
            .collect()
    }

    /// The group cards that list this contact UID.
    pub fn groups_with(&self, uid: &str) -> Vec<Card> {
        self.cards
            .values()
            .filter(|c| {
                c.group
                    .as_ref()
                    .is_some_and(|g| g.members.iter().any(|m| m == uid))
            })
            .cloned()
            .collect()
    }

    /// PUTs a card and records it: iCloud stores the bytes as sent, so the index keeps the card
    /// with the returned ETag and a later delta with that ETag needs no fetch.
    pub async fn put(
        &mut self,
        client: &DavClient,
        href: &str,
        card: Component,
        pre: Precondition,
    ) -> Result<()> {
        let book = self.book()?.clone();
        let text = card.to_ics();
        check_size(&text, book.max_resource_size)?;
        let etag = client.put(&book.card_url(href)?, text, pre).await?;
        match etag {
            Some(etag) => self.put_parsed(href.to_string(), etag, card),
            None => self.remove(href),
        }
        Ok(())
    }

    /// DELETEs a card with `If-Match` (which iCloud does not enforce) and forgets it.
    pub async fn delete(&mut self, client: &DavClient, href: &str, etag: &str) -> Result<()> {
        let url = self.book()?.card_url(href)?;
        client.delete(&url, etag).await?;
        self.remove(href);
        Ok(())
    }
}

/// The path of a card in the book from an href, or None for the book itself and anything that is
/// not directly inside it.
fn card_path(book: &Book, href: &str) -> Result<Option<String>> {
    let path = book.path(href)?;
    let base = book.url.path();
    let inside = path
        .strip_prefix(base)
        .is_some_and(|name| !name.is_empty() && !name.contains('/'));
    Ok(inside.then_some(path))
}

/// The CardDAV client and the index, shared by every MCP session.
pub struct Store {
    client: Arc<DavClient>,
    index: Mutex<Index>,
}

impl Store {
    pub fn new(client: Arc<DavClient>) -> Self {
        Store {
            client,
            index: Mutex::new(Index::default()),
        }
    }

    pub fn client(&self) -> &DavClient {
        &self.client
    }

    /// Locks the index and brings it up to date. The lock is held for the whole tool call, so the
    /// service never races itself.
    pub async fn open(&self) -> Result<MutexGuard<'_, Index>> {
        let mut index = self.index.lock().await;
        index.refresh(&self.client).await?;
        Ok(index)
    }
}
