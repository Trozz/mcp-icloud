use crate::error::{self, MailError};
use crate::imap::proto::quote;
use crate::imap::{ImapClient, MessageSummary, Role, SearchQuery};
use crate::mail::build::Identity;
use crate::mail::parse::{self, BodyFormat};
use crate::smtp::Sender;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mcp_server_core::observability::content_logging_enabled;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ResourceContents, ServerCapabilities,
    ServerConfig,
};
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;

const DEFAULT_LIMIT: u32 = 25;
const DEFAULT_MAX_CHARS: usize = 20_000;
const THREAD_MAX_IDS: usize = 20;
const THREAD_MAX_PER_MAILBOX: usize = 50;

const INSTRUCTIONS: &str = "Tools for one iCloud Mail account. Messages are addressed by mailbox name plus uid. \
Every listing returns the mailbox uidvalidity; pass it back to mutating tools so they refuse to act if the uids went stale. \
Compose tools only create drafts in the Drafts mailbox for the user to review. Sending is only possible with send_draft, which exists only when the server allows sending.";

pub struct ToolSettings {
    pub identity: Identity,
    pub max_attachment_bytes: usize,
}

#[derive(Clone)]
pub struct MailServer {
    imap: Arc<ImapClient>,
    settings: Arc<ToolSettings>,
    tool_router: ToolRouter<MailServer>,
}

fn default_inbox() -> String {
    "INBOX".into()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchParams {
    /// Mailbox to search. Default INBOX.
    #[serde(default = "default_inbox")]
    pub mailbox: String,
    /// Text that must appear in the From header.
    pub from: Option<String>,
    /// Text that must appear in the To header.
    pub to: Option<String>,
    /// Text that must appear in the subject.
    pub subject: Option<String>,
    /// Text that must appear anywhere in the message.
    pub text: Option<String>,
    /// Only messages on or after this date, YYYY-MM-DD.
    pub since: Option<String>,
    /// Only messages before this date, YYYY-MM-DD.
    pub before: Option<String>,
    /// true: only unread messages. false: only read messages.
    pub unseen: Option<bool>,
    /// true: only flagged messages. false: only unflagged messages.
    pub flagged: Option<bool>,
    /// Page size, 1 to 100. Default 25.
    pub limit: Option<u32>,
    /// next_cursor from a previous page.
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetMessageParams {
    /// Mailbox the message is in. Default INBOX.
    #[serde(default = "default_inbox")]
    pub mailbox: String,
    pub uid: u32,
    /// text (default), html or raw.
    pub format: Option<String>,
    /// Maximum body characters to return. Default 20000.
    pub max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetAttachmentParams {
    /// Mailbox the message is in. Default INBOX.
    #[serde(default = "default_inbox")]
    pub mailbox: String,
    pub uid: u32,
    /// Attachment index from get_message.
    pub index: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MessageRef {
    /// Mailbox the message is in. Default INBOX.
    #[serde(default = "default_inbox")]
    pub mailbox: String,
    pub uid: u32,
}

#[derive(Serialize)]
struct ThreadEntry {
    mailbox: String,
    #[serde(flatten)]
    message: MessageSummary,
}

fn json_result(v: &serde_json::Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// Runs one tool: span, metrics, and mapping of `MailError` to a tool error (`isError: true`).
async fn run(
    tool: &'static str,
    fut: impl Future<Output = error::Result<CallToolResult>>,
) -> Result<CallToolResult, McpError> {
    let started = Instant::now();
    let result = fut.instrument(tracing::info_span!("tool", tool)).await;
    let outcome = if result.is_ok() { "ok" } else { "error" };
    metrics::counter!("mcp_tool_calls_total", "tool" => tool, "outcome" => outcome).increment(1);
    metrics::histogram!("mcp_tool_duration_seconds", "tool" => tool)
        .record(started.elapsed().as_secs_f64());
    tracing::info!(
        tool,
        outcome,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "tool call"
    );
    match result {
        Ok(r) => {
            if content_logging_enabled() {
                tracing::debug!(tool, result = ?r.content, "tool result");
            }
            Ok(r)
        }
        Err(e) => {
            tracing::warn!(tool, error = %e, "tool failed");
            Ok(CallToolResult::error(vec![ContentBlock::text(
                e.to_string(),
            )]))
        }
    }
}

fn uri_component(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('/', "%2F")
}

/// `OR (a) (b) ...` over every id: the message itself, its replies and anything it references.
fn thread_criteria(ids: &[String]) -> error::Result<Option<String>> {
    let mut parts = Vec::new();
    for id in ids {
        let q = quote(&format!("<{id}>"))?;
        parts.push(format!(
            "OR HEADER Message-ID {q} OR HEADER References {q} HEADER In-Reply-To {q}"
        ));
    }
    Ok(parts.into_iter().reduce(|a, b| format!("OR ({a}) ({b})")))
}

impl MailServer {
    pub fn new(
        imap: Arc<ImapClient>,
        _sender: Option<Arc<dyn Sender>>,
        settings: Arc<ToolSettings>,
    ) -> Self {
        Self {
            imap,
            settings,
            tool_router: Self::read_router(),
        }
    }

    /// Names of the registered tools, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    async fn thread(&self, p: MessageRef) -> error::Result<CallToolResult> {
        let (_, raw) = self.imap.fetch_raw(&p.mailbox, p.uid, None).await?;
        let v = parse::view(&raw, BodyFormat::Text, 0)?;
        let mut ids: Vec<String> = Vec::new();
        for id in v
            .references
            .iter()
            .chain(&v.in_reply_to)
            .chain(&v.message_id)
        {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        // Keep the most recent ids: References lists the oldest ancestors first.
        let ids: Vec<String> = ids.into_iter().rev().take(THREAD_MAX_IDS).collect();
        let Some(criteria) = thread_criteria(&ids)? else {
            let (_, s) = self.imap.summaries(&p.mailbox, &[p.uid]).await?;
            let entries: Vec<ThreadEntry> = s
                .into_iter()
                .map(|message| ThreadEntry {
                    mailbox: p.mailbox.clone(),
                    message,
                })
                .collect();
            return Ok(json_result(&json!({ "messages": entries })));
        };
        let mut mailboxes = vec![p.mailbox.clone()];
        if let Ok(sent) = self.imap.role_mailbox(Role::Sent).await
            && sent != p.mailbox
        {
            mailboxes.push(sent);
        }
        let mut entries = Vec::new();
        for mailbox in mailboxes {
            let mut uids = self.imap.search_uids(&mailbox, &criteria).await?;
            uids.truncate(THREAD_MAX_PER_MAILBOX);
            if uids.is_empty() {
                continue;
            }
            let (_, summaries) = self.imap.summaries(&mailbox, &uids).await?;
            entries.extend(summaries.into_iter().map(|message| ThreadEntry {
                mailbox: mailbox.clone(),
                message,
            }));
        }
        entries.sort_by(|a, b| a.message.date.cmp(&b.message.date));
        Ok(json_result(&json!({ "messages": entries })))
    }
}

#[tool_router(router = read_router)]
impl MailServer {
    #[tool(
        description = "List mailboxes with their role (inbox, sent, drafts, trash, junk, archive), total and unread counts.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn list_mailboxes(&self) -> Result<CallToolResult, McpError> {
        run("list_mailboxes", async {
            let boxes = self.imap.mailboxes().await?;
            Ok(json_result(&json!({ "mailboxes": boxes })))
        })
        .await
    }

    #[tool(
        description = "Search a mailbox, newest first. All filters are optional and combined with AND. Returns message summaries, the mailbox uidvalidity and next_cursor for the next page.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn search_messages(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "search_messages");
        run("search_messages", async {
            let q = SearchQuery {
                from: p.from,
                to: p.to,
                subject: p.subject,
                text: p.text,
                since: p.since,
                before: p.before,
                unseen: p.unseen,
                flagged: p.flagged,
            };
            let limit = p.limit.unwrap_or(DEFAULT_LIMIT) as usize;
            let page = self
                .imap
                .search(&p.mailbox, &q, limit, p.cursor.as_deref())
                .await?;
            Ok(json_result(&json!(page)))
        })
        .await
    }

    #[tool(
        description = "Read one message: headers, body (text by default, html or raw on request) and the attachment list. Does not mark the message as read.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_message(
        &self,
        Parameters(p): Parameters<GetMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "get_message");
        run("get_message", async {
            let format = BodyFormat::parse(p.format.as_deref())?;
            let (uidvalidity, raw) = self.imap.fetch_raw(&p.mailbox, p.uid, None).await?;
            let view = parse::view(&raw, format, p.max_chars.unwrap_or(DEFAULT_MAX_CHARS))?;
            Ok(json_result(&json!({
                "mailbox": p.mailbox,
                "uid": p.uid,
                "uidvalidity": uidvalidity,
                "message": view,
            })))
        })
        .await
    }

    #[tool(
        description = "Download one attachment (index from get_message) as an embedded base64 resource.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_attachment(
        &self,
        Parameters(p): Parameters<GetAttachmentParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "get_attachment");
        run("get_attachment", async {
            let (_, raw) = self.imap.fetch_raw(&p.mailbox, p.uid, None).await?;
            let a = parse::attachment(&raw, p.index)?;
            let max = self.settings.max_attachment_bytes;
            if a.contents.len() > max {
                return Err(MailError::Invalid(format!(
                    "the attachment is {} bytes, larger than MAX_ATTACHMENT_BYTES ({max})",
                    a.contents.len()
                )));
            }
            let uri = format!(
                "icloud-mail://{}/{}/attachments/{}",
                uri_component(&p.mailbox),
                p.uid,
                p.index
            );
            let meta = json!({
                "mailbox": p.mailbox,
                "uid": p.uid,
                "index": p.index,
                "filename": a.filename,
                "mime_type": a.mime_type,
                "size": a.contents.len(),
                "uri": uri,
            });
            let blob = ResourceContents::blob(STANDARD.encode(&a.contents), uri)
                .with_mime_type(a.mime_type);
            Ok(CallToolResult::success(vec![
                ContentBlock::text(meta.to_string()),
                ContentBlock::resource(blob),
            ]))
        })
        .await
    }

    #[tool(
        description = "Find the conversation a message belongs to (linked by Message-ID, References and In-Reply-To), searching its mailbox and Sent. Ordered by date.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_thread(
        &self,
        Parameters(p): Parameters<MessageRef>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(args = ?p, "get_thread");
        run("get_thread", self.thread(p)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MailServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                crate::config::SERVICE_NAME,
                crate::VERSION,
            ))
            .with_instructions(INSTRUCTIONS)
    }
}
