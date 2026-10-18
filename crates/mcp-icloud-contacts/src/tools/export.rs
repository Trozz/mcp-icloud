//! export_vcard: a contact as vCard text, the card exactly as stored.

use super::read::IdParams;
use super::{ContactsServer, run};
use crate::error;
use crate::index::Index;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde_json::json;

#[tool_router(router = export_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Export a contact as vCard (.vcf) text, the card exactly as iCloud stores it. Returns a summary (id, kind, display_name, cards, missing_members, bytes) and the text as a text/vcard resource.",
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

/// The export of one contact: the stored line tree written back unchanged.
fn export(idx: &Index, id: &str) -> error::Result<CallToolResult> {
    let (_, tree) = idx.contact_card(id)?;
    let text = tree.to_ics();
    let name = idx.contact(id)?.display_name;
    Ok(vcf_result(id, "contact", &name, text, 1, &[]))
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
