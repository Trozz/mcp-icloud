//! merge_contacts.

use super::{ContactsServer, json_result, run};
use crate::error::{self, ContactsError};
use crate::index::Card;
use crate::vcard::merge::merge_fields;
use crate::vcard::model::Contact;
use crate::vcard::patch::{add_members, apply, remove_members, touch};
use chrono::Utc;
use mcp_icloud_dav::client::Precondition;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

/// At most this many contacts merged in one call.
pub const MAX_MERGE: usize = 10;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MergeParams {
    /// id of the contact to keep (the first contact of a find_duplicate_contacts group is a good choice).
    pub keep_id: String,
    /// ids of the contacts to merge into it and then delete, 1 to 10.
    pub merge_ids: Vec<String>,
}

/// A merged contact as read at the start of the call.
struct Merged {
    id: String,
    href: String,
    etag: String,
    contact: Contact,
}

fn check_ids(p: &MergeParams) -> error::Result<Vec<String>> {
    let keep = p.keep_id.trim();
    let ids: Vec<String> = p.merge_ids.iter().map(|i| i.trim().to_string()).collect();
    if ids.is_empty() {
        return Err(ContactsError::invalid("merge_ids is empty"));
    }
    if ids.len() > MAX_MERGE {
        return Err(ContactsError::invalid(format!(
            "merge_ids has {} ids; at most {MAX_MERGE} can be merged at once",
            ids.len()
        )));
    }
    for (i, id) in ids.iter().enumerate() {
        if id == keep {
            return Err(ContactsError::invalid(format!(
                "merge_ids contains keep_id {id}"
            )));
        }
        if ids[..i].contains(id) {
            return Err(ContactsError::invalid(format!(
                "merge_ids lists {id} twice"
            )));
        }
    }
    Ok(ids)
}

#[tool_router(router = merge_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Merge contacts into one: emails, phones, URLs, addresses, dates and related names the kept contact lacks are added, its empty fields (organization, department, job title, nickname, birthday, and the name when it has none) are filled, notes are combined, groups move to the kept contact, then the merged contacts are deleted. Photos are not copied: photo_not_kept lists merged contacts whose photo was dropped (use copy_contact_photo first, or keep the contact with the photo). The owner's own card can be kept but not merged away.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn merge_contacts(
        &self,
        Parameters(p): Parameters<MergeParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.keep_id, merging = p.merge_ids.len(), "merge_contacts");
        run("merge_contacts", self.merge(p)).await
    }
}

impl ContactsServer {
    /// Everything is checked before the first write. The kept card is written first: if that
    /// fails nothing has changed. Then each group that lists a merged contact gets the kept one
    /// instead, and the merged contacts are deleted right after this call's refresh (iCloud does
    /// not enforce `If-Match` on DELETE). Failures after the first write are reported, not undone.
    async fn merge(&self, p: MergeParams) -> error::Result<CallToolResult> {
        let ids = check_ids(&p)?;
        let mut idx = self.store.open().await?;
        let (card, tree) = idx.contact_card(&p.keep_id)?;
        let (keep_href, keep_etag, mut tree) = (card.href.clone(), card.etag.clone(), tree.clone());
        let keep_uid = card.uid.clone().unwrap_or_default();
        let kept = idx.contact(&keep_uid)?;
        let mut merged = Vec::new();
        for id in &ids {
            let (card, _) = idx.contact_card(id)?;
            if idx.is_me(card) {
                return Err(ContactsError::invalid(format!(
                    "{id} is the owner's own card (My Card); keep it and merge the others into it"
                )));
            }
            merged.push(Merged {
                id: card.uid.clone().unwrap_or_default(),
                href: card.href.clone(),
                etag: card.etag.clone(),
                contact: idx.contact(id)?,
            });
        }
        let others: Vec<&Contact> = merged.iter().map(|m| &m.contact).collect();
        let before = tree.clone();
        apply(&mut tree, &merge_fields(&kept, &others))?;
        if tree != before {
            touch(&mut tree, Utc::now());
            idx.put(
                self.store.client(),
                &keep_href,
                tree,
                Precondition::IfMatch(keep_etag),
            )
            .await?;
        }
        tracing::info!(id = %keep_uid, merged = merged.len(), "contacts merged");

        let merged_uids: Vec<String> = merged.iter().map(|m| m.id.clone()).collect();
        let mut groups: Vec<Card> = Vec::new();
        for uid in &merged_uids {
            for g in idx.groups_with(uid) {
                if !groups.iter().any(|x| x.href == g.href) {
                    groups.push(g);
                }
            }
        }
        let mut groups_updated = Vec::new();
        let mut groups_not_updated = Vec::new();
        for group in groups {
            let gid = group.uid.clone().unwrap_or_default();
            let Some(mut tree) = group.card.clone() else {
                continue;
            };
            add_members(&mut tree, std::slice::from_ref(&keep_uid));
            remove_members(&mut tree, &merged_uids);
            touch(&mut tree, Utc::now());
            let pre = Precondition::IfMatch(group.etag.clone());
            match idx.put(self.store.client(), &group.href, tree, pre).await {
                Ok(()) => groups_updated.push(gid),
                Err(e) => {
                    tracing::warn!(group = %gid, kind = e.kind(), "a group still lists a merged contact");
                    groups_not_updated
                        .push(json!({"id": gid, "error": format!("{}: {e}", e.kind())}));
                }
            }
        }

        let mut not_deleted = Vec::new();
        for m in &merged {
            if let Err(e) = idx.delete(self.store.client(), &m.href, &m.etag).await {
                tracing::warn!(id = %m.id, kind = e.kind(), "a merged contact was not deleted");
                not_deleted.push(json!({"id": m.id, "error": format!("{}: {e}", e.kind())}));
            }
        }
        let photo_not_kept: Vec<&String> = merged
            .iter()
            .filter(|m| m.contact.has_photo && !kept.has_photo)
            .map(|m| &m.id)
            .collect();
        if idx.contact(&keep_uid).is_err()
            && let Err(e) = idx.refresh(self.store.client()).await
        {
            tracing::warn!(kind = e.kind(), "the index was not refreshed after a merge");
        }
        let contact: Value = idx
            .contact(&keep_uid)
            .ok()
            .and_then(|c| serde_json::to_value(c).ok())
            .unwrap_or_default();
        Ok(json_result(&json!({
            "id": keep_uid,
            "merged": merged_uids,
            "groups_updated": groups_updated,
            "groups_not_updated": groups_not_updated,
            "not_deleted": not_deleted,
            "photo_not_kept": photo_not_kept,
            "contact": contact,
        })))
    }
}
