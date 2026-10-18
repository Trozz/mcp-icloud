//! export_vcard: a contact, or every member of a group, as vCard text, the cards exactly as stored.

use super::read::IdParams;
use super::{ContactsServer, run};
use crate::error;
use crate::index::Index;
use crate::vcard::parse::Kind;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde_json::json;
use std::collections::HashSet;

#[tool_router(router = export_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Export a contact or a group (iOS List) as vCard (.vcf) text. A contact's card comes exactly as iCloud stores it; a group gives every member's card in member order, and members that are not readable contacts are listed under missing_members. Returns a summary (id, kind, display_name, cards, missing_members, bytes) and the text as a text/vcard resource.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn export_vcard(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(id = %p.id, "export_vcard");
        run("export_vcard", async {
            let idx = self.store.open().await?;
            export(&idx, p.id.trim())
        })
        .await
    }
}

/// The export of a contact (its stored line tree written back unchanged) or of a group.
fn export(idx: &Index, id: &str) -> error::Result<CallToolResult> {
    if idx.card(id)?.kind == Kind::Group {
        return group_export(idx, id);
    }
    let (_, tree) = idx.contact_card(id)?;
    let text = tree.to_ics();
    let name = idx.contact(id)?.display_name;
    Ok(vcf_result(id, "contact", &name, text, 1, &[]))
}

/// Every member's card in member order, each member once. Member ids that are not parsed contact
/// cards (deleted contacts, groups, unparseable cards) are skipped and reported.
fn group_export(idx: &Index, id: &str) -> error::Result<CallToolResult> {
    let group = idx.group(id)?;
    let mut text = String::new();
    let mut cards = 0;
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for uid in &group.members {
        if !seen.insert(uid) {
            continue;
        }
        let tree = idx
            .card(uid)
            .ok()
            .filter(|c| c.kind == Kind::Contact)
            .and_then(|c| c.card.as_ref());
        let Some(tree) = tree else {
            missing.push(uid.clone());
            continue;
        };
        text.push_str(&tree.to_ics());
        cards += 1;
    }
    Ok(vcf_result(id, "group", &group.name, text, cards, &missing))
}

/// The summary first, as tool clients read the first block, then the vCard text as a resource.
fn vcf_result(
    id: &str,
    kind: &str,
    display_name: &str,
    text: String,
    cards: usize,
    missing_members: &[String],
) -> CallToolResult {
    let meta = json!({
        "id": id,
        "kind": kind,
        "display_name": display_name,
        "cards": cards,
        "missing_members": missing_members,
        "bytes": text.len(),
    });
    let uri = format!("icloud-contacts://export/{}.vcf", uri_component(id));
    let vcf = ResourceContents::text(text, uri).with_mime_type("text/vcard");
    CallToolResult::success(vec![
        ContentBlock::text(meta.to_string()),
        ContentBlock::resource(vcf),
    ])
}

/// `%`, space and `/` percent-encoded, as the mail service writes its resource URIs.
fn uri_component(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('/', "%2F")
}
