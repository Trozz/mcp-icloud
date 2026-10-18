//! copy_contact_photo.

use super::read::contact_result;
use super::{ContactsServer, run};
use crate::error::{self, ContactsError};
use crate::vcard::patch::touch;
use crate::vcard::photo::copy_photo;
use chrono::Utc;
use mcp_icloud_dav::client::Precondition;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CopyPhotoParams {
    /// id of the contact whose photo is copied.
    pub from_id: String,
    /// id of the contact that gets the photo; its own photo, if any, is replaced.
    pub to_id: String,
}

#[tool_router(router = photo_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Copy one contact's photo onto another, replacing the other's photo. Use it before merge_contacts when the contact to keep has no photo but a duplicate has one.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn copy_contact_photo(
        &self,
        Parameters(p): Parameters<CopyPhotoParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(from = %p.from_id, to = %p.to_id, "copy_contact_photo");
        run("copy_contact_photo", self.copy_photo(p)).await
    }
}

impl ContactsServer {
    async fn copy_photo(&self, p: CopyPhotoParams) -> error::Result<CallToolResult> {
        if p.from_id.trim() == p.to_id.trim() {
            return Err(ContactsError::invalid(
                "from_id and to_id are the same contact",
            ));
        }
        let mut idx = self.store.open().await?;
        let (_, from) = idx.contact_card(&p.from_id)?;
        let from = from.clone();
        let (card, tree) = idx.contact_card(&p.to_id)?;
        let (href, etag, mut tree) = (card.href.clone(), card.etag.clone(), tree.clone());
        let uid = card.uid.clone().unwrap_or_default();
        let before = tree.clone();
        if !copy_photo(&from, &mut tree) {
            return Err(ContactsError::invalid(format!(
                "{} has no photo",
                p.from_id.trim()
            )));
        }
        if tree != before {
            touch(&mut tree, Utc::now());
            idx.put(
                self.store.client(),
                &href,
                tree,
                Precondition::IfMatch(etag),
            )
            .await?;
            tracing::info!(id = %uid, "contact photo copied");
        }
        Ok(contact_result(&idx.contact(&uid)?))
    }
}
