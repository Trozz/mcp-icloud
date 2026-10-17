//! search_contacts, get_contact, get_my_contact and list_groups.

use super::{ContactsServer, json_result, run};
use crate::error::{self, ContactsError};
use crate::search;
use crate::vcard::model::Contact;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

pub const DEFAULT_LIMIT: u32 = 25;
pub const MAX_LIMIT: u32 = 100;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchParams {
    /// Words to find in names, nickname, organization, department, job title, emails and related names, or a phone number in any format. Omit to list everyone.
    pub query: Option<String>,
    /// Only members of this group (id from list_groups).
    pub group_id: Option<String>,
    /// At most this many results, 1 to 100. Default 25.
    pub limit: Option<u32>,
    /// Skip this many results to get the next page. Default 0.
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct IdParams {
    /// id from search_contacts or list_groups.
    pub id: String,
}

/// The compact form of a contact in search results.
pub(crate) fn summary(c: &Contact) -> Value {
    let mut v = json!({
        "id": c.id,
        "display_name": c.display_name,
        "emails": c.emails.iter().map(|e| &e.value).collect::<Vec<_>>(),
        "phones": c.phones.iter().map(|e| &e.value).collect::<Vec<_>>(),
        "is_me": c.is_me,
    });
    if let Some(org) = &c.organization {
        v["organization"] = json!(org);
    }
    v
}

pub(crate) fn contact_result(c: &Contact) -> CallToolResult {
    json_result(&serde_json::to_value(c).unwrap_or_default())
}

#[tool_router(router = read_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Find contacts by name, nickname, organization, department, job title, email or related name (every word must match; case and accents are ignored), or by a phone number in any format. Without query, lists everyone in name order. group_id limits the search to one group. Returns the total and one page of summaries.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn search_contacts(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(
            has_query = p.query.is_some(),
            group = p.group_id.is_some(),
            "search_contacts"
        );
        run("search_contacts", self.search(p)).await
    }

    #[tool(
        description = "Everything about one contact: names, organization, emails, phones, addresses, URLs, birthday, dates, related names, social profiles, instant messaging, note and groups.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_contact(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.id, "get_contact");
        run("get_contact", async {
            let idx = self.store.open().await?;
            Ok(contact_result(&idx.contact(&p.id)?))
        })
        .await
    }

    #[tool(
        description = "The owner's own card (My Card in iOS Contacts), in full.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_my_contact(&self) -> Result<CallToolResult, McpError> {
        run("get_my_contact", async {
            let idx = self.store.open().await?;
            let me = idx.me().ok_or_else(|| {
                ContactsError::NotFound("no My Card is set in iCloud Contacts".into())
            })?;
            let id = me.uid.clone().unwrap_or_default();
            Ok(contact_result(&idx.contact(&id)?))
        })
        .await
    }

    #[tool(
        description = "Every group (iOS List) with its id, name and number of members.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn list_groups(&self) -> Result<CallToolResult, McpError> {
        run("list_groups", async {
            let idx = self.store.open().await?;
            let groups: Vec<Value> = idx
                .groups()
                .iter()
                .map(|g| {
                    json!({
                        "id": g.id,
                        "name": g.name,
                        "member_count": idx.known_members(g).len(),
                    })
                })
                .collect();
            Ok(json_result(&Value::Array(groups)))
        })
        .await
    }
}

impl ContactsServer {
    async fn search(&self, p: SearchParams) -> error::Result<CallToolResult> {
        let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ContactsError::invalid(format!(
                "limit must be between 1 and {MAX_LIMIT}"
            )));
        }
        let offset = p.offset.unwrap_or(0) as usize;
        let idx = self.store.open().await?;
        let mut contacts = idx.contacts();
        if let Some(id) = &p.group_id {
            let group = idx.group(id)?;
            contacts.retain(|c| group.members.contains(&c.id));
        }
        let found = search::search(&contacts, p.query.as_deref());
        let page: Vec<Value> = found
            .iter()
            .skip(offset)
            .take(limit as usize)
            .map(|c| summary(c))
            .collect();
        Ok(json_result(&json!({
            "total": found.len(),
            "offset": offset,
            "contacts": page,
        })))
    }
}
