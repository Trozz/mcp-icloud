//! create_group, update_group and delete_group. A group is an iOS List: a card in the address book
//! that lists its members by contact UID.

use super::read::IdParams;
use super::write::new_uid;
use super::{ContactsServer, json_result, run};
use crate::error::{self, ContactsError};
use crate::index::Index;
use crate::vcard::build::new_group;
use crate::vcard::parse::{GROUP_MEMBER, Kind, member_uid, named};
use crate::vcard::patch::{add_members, remove_members, rename_group, touch};
use chrono::Utc;
use mcp_icloud_dav::client::Precondition;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateGroupParams {
    /// The group's name.
    pub name: String,
    /// Contact ids (from search_contacts) to put in the group.
    pub member_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateGroupParams {
    /// id from list_groups.
    pub id: String,
    /// A new name.
    pub name: Option<String>,
    /// Contact ids to add.
    pub add_ids: Option<Vec<String>>,
    /// Contact ids to remove.
    pub remove_ids: Option<Vec<String>>,
}

/// A group with its known members.
fn group_result(idx: &Index, id: &str) -> error::Result<CallToolResult> {
    let group = idx.group(id)?;
    let members: Vec<Value> = idx
        .known_members(&group)
        .iter()
        .map(|c| json!({"id": c.id, "display_name": c.display_name}))
        .collect();
    Ok(json_result(&json!({
        "id": group.id,
        "name": group.name,
        "member_count": members.len(),
        "members": members,
    })))
}

/// Every id must name a known contact (not a group); duplicates are dropped.
fn contact_ids(idx: &Index, ids: &[String], field: &str) -> error::Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for id in ids.iter().map(|i| i.trim()) {
        let known = idx
            .card(id)
            .is_ok_and(|c| c.kind == Kind::Contact && c.contact.is_some());
        if !known {
            return Err(ContactsError::invalid(format!(
                "{field} contains {id}, which is not a contact id"
            )));
        }
        if !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

fn group_name(name: &str) -> error::Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ContactsError::invalid("name is empty"));
    }
    Ok(name)
}

#[tool_router(router = group_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Create a group (an iOS List) with a name and optional member contact ids.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn create_group(
        &self,
        Parameters(p): Parameters<CreateGroupParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("create_group");
        run("create_group", async {
            let name = group_name(&p.name)?;
            let mut idx = self.store.open().await?;
            let members = contact_ids(&idx, &p.member_ids.unwrap_or_default(), "member_ids")?;
            let uid = new_uid();
            let card = new_group(&uid, name, &members, Utc::now());
            let href = idx.book()?.new_card_path(&uid);
            idx.put(self.store.client(), &href, card, Precondition::IfNoneMatch)
                .await?;
            tracing::info!(id = %uid, "group created");
            group_result(&idx, &uid)
        })
        .await
    }

    #[tool(
        description = "Rename a group and add or remove members by contact id. Removing a contact from a group does not delete the contact.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn update_group(
        &self,
        Parameters(p): Parameters<UpdateGroupParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.id, "update_group");
        run("update_group", async {
            if p.name.is_none() && p.add_ids.is_none() && p.remove_ids.is_none() {
                return Err(ContactsError::invalid(
                    "nothing to change: pass name, add_ids or remove_ids",
                ));
            }
            let name = p.name.as_deref().map(group_name).transpose()?;
            let mut idx = self.store.open().await?;
            let (card, tree) = idx.group_card(&p.id)?;
            let (href, etag, mut tree) = (card.href.clone(), card.etag.clone(), tree.clone());
            let add = contact_ids(&idx, &p.add_ids.unwrap_or_default(), "add_ids")?;
            let listed: Vec<String> = named(&tree, GROUP_MEMBER)
                .filter_map(|(_, m)| member_uid(m.value()))
                .collect();
            let remove: Vec<String> = p
                .remove_ids
                .unwrap_or_default()
                .iter()
                .map(|i| i.trim().to_string())
                .collect();
            if let Some(id) = remove.iter().find(|id| !listed.contains(id)) {
                return Err(ContactsError::invalid(format!(
                    "remove_ids contains {id}, which the group does not list"
                )));
            }
            if let Some(name) = name {
                rename_group(&mut tree, name);
            }
            add_members(&mut tree, &add);
            remove_members(&mut tree, &remove);
            touch(&mut tree, Utc::now());
            idx.put(
                self.store.client(),
                &href,
                tree,
                Precondition::IfMatch(etag),
            )
            .await?;
            tracing::info!(id = %p.id, "group updated");
            group_result(&idx, &p.id)
        })
        .await
    }

    #[tool(
        description = "Delete a group. Its member contacts are kept.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn delete_group(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.id, "delete_group");
        run("delete_group", async {
            let mut idx = self.store.open().await?;
            let (card, _) = idx.group_card(&p.id)?;
            let (href, etag) = (card.href.clone(), card.etag.clone());
            idx.delete(self.store.client(), &href, &etag).await?;
            tracing::info!(id = %p.id, "group deleted");
            Ok(json_result(&json!({"deleted": true, "id": p.id.trim()})))
        })
        .await
    }
}
