//! find_duplicate_contacts.

use super::read::summary;
use super::{ContactsServer, json_result, run};
use crate::duplicates;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde_json::{Value, json};

#[tool_router(router = duplicate_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Find likely duplicate contacts: contacts that share an email address, a phone number (in any format) or a full name of at least two words. Returns groups with the reasons they matched; in each group the first contact is the best one to keep (it has a photo, or the most details). Use merge_contacts to merge a group.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn find_duplicate_contacts(&self) -> Result<CallToolResult, McpError> {
        run("find_duplicate_contacts", async {
            let idx = self.store.open().await?;
            let contacts = idx.contacts();
            let groups: Vec<Value> = duplicates::find(&contacts)
                .into_iter()
                .map(|g| {
                    let members: Vec<Value> = g
                        .members
                        .iter()
                        .map(|&i| {
                            let c = &contacts[i];
                            let mut v = summary(c);
                            v["has_photo"] = json!(c.has_photo);
                            if let Some(u) = &c.updated {
                                v["updated"] = json!(u);
                            }
                            v
                        })
                        .collect();
                    json!({
                        "reasons": g.reasons.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
                        "contacts": members,
                    })
                })
                .collect();
            tracing::info!(groups = groups.len(), "duplicates found");
            Ok(json_result(&json!({"groups": groups})))
        })
        .await
    }
}
