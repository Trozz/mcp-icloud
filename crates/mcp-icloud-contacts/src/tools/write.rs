//! create_contact, update_contact and delete_contact.

use super::read::{IdParams, contact_result};
use super::{ContactsServer, json_result, run};
use crate::error::{self, ContactsError};
use crate::vcard::build::new_contact;
use crate::vcard::model::ContactFields;
use crate::vcard::patch::{apply, remove_members, touch};
use chrono::Utc;
use mcp_icloud_dav::client::Precondition;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateContactParams {
    /// id from search_contacts.
    pub id: String,
    #[serde(flatten)]
    pub fields: ContactFields,
}

/// A new card's UID, also its file name: an upper-case UUID like the ones iOS writes.
pub(crate) fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

#[tool_router(router = write_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Create a contact. Needs given_name, family_name, display_name or organization. Lists (emails, phones, urls, addresses, dates, related) take plain labels such as home, work, mobile, iPhone, home page or mother; any other label is kept as custom text.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn create_contact(
        &self,
        Parameters(p): Parameters<ContactFields>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("create_contact");
        run("create_contact", self.create(p)).await
    }

    #[tool(
        description = "Change a contact. Only the fields you pass change, and \"\" clears one. A list you pass replaces that whole list ([] clears it): include the entries to keep exactly as get_contact returned them and they stay untouched. Photos and anything this server does not show are kept.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn update_contact(
        &self,
        Parameters(p): Parameters<UpdateContactParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.id, "update_contact");
        run("update_contact", self.update(p)).await
    }

    #[tool(
        description = "Delete a contact and remove it from every group. The owner's own card (My Card) cannot be deleted.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn delete_contact(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.id, "delete_contact");
        run("delete_contact", self.delete(p)).await
    }
}

impl ContactsServer {
    async fn create(&self, fields: ContactFields) -> error::Result<CallToolResult> {
        let mut idx = self.store.open().await?;
        let uid = new_uid();
        let card = new_contact(&uid, &fields, Utc::now())?;
        let href = idx.book()?.new_card_path(&uid);
        idx.put(self.store.client(), &href, card, Precondition::IfNoneMatch)
            .await?;
        tracing::info!(id = %uid, "contact created");
        Ok(contact_result(&idx.contact(&uid)?))
    }

    async fn update(&self, p: UpdateContactParams) -> error::Result<CallToolResult> {
        if p.fields.is_empty() {
            return Err(ContactsError::invalid(
                "nothing to change: pass at least one field",
            ));
        }
        let mut idx = self.store.open().await?;
        let (card, tree) = idx.contact_card(&p.id)?;
        let (href, etag, mut tree) = (card.href.clone(), card.etag.clone(), tree.clone());
        apply(&mut tree, &p.fields)?;
        touch(&mut tree, Utc::now());
        idx.put(
            self.store.client(),
            &href,
            tree,
            Precondition::IfMatch(etag),
        )
        .await?;
        tracing::info!(id = %p.id, "contact updated");
        Ok(contact_result(&idx.contact(&p.id)?))
    }

    /// The refresh at the start of the call is the fresh read: iCloud does not enforce `If-Match`
    /// on DELETE, so the DELETE follows it immediately. Groups are cleaned afterwards; a group
    /// that cannot be updated keeps a member UID that every view ignores.
    async fn delete(&self, p: IdParams) -> error::Result<CallToolResult> {
        let mut idx = self.store.open().await?;
        let (card, _) = idx.contact_card(&p.id)?;
        if idx.is_me(card) {
            return Err(ContactsError::invalid(
                "this is the owner's own card (My Card); it cannot be deleted",
            ));
        }
        let (href, etag) = (card.href.clone(), card.etag.clone());
        let uid = card.uid.clone().unwrap_or_default();
        idx.delete(self.store.client(), &href, &etag).await?;
        tracing::info!(id = %uid, "contact deleted");
        let mut updated = Vec::new();
        let mut not_updated = Vec::new();
        for group in idx.groups_with(&uid) {
            let gid = group.uid.clone().unwrap_or_default();
            let Some(mut tree) = group.card.clone() else {
                continue;
            };
            remove_members(&mut tree, std::slice::from_ref(&uid));
            touch(&mut tree, Utc::now());
            let pre = Precondition::IfMatch(group.etag.clone());
            match idx.put(self.store.client(), &group.href, tree, pre).await {
                Ok(()) => updated.push(gid),
                Err(e) => {
                    tracing::warn!(group = %gid, kind = e.kind(), "a group still lists a deleted contact");
                    not_updated.push(json!({"id": gid, "error": format!("{}: {e}", e.kind())}));
                }
            }
        }
        Ok(json_result(&json!({
            "deleted": true,
            "id": uid,
            "groups_updated": updated,
            "groups_not_updated": not_updated,
        })))
    }
}
