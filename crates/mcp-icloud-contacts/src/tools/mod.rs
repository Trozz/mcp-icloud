//! The MCP server of the contacts service.

use crate::error;
use crate::index::Store;
use chrono_tz::Tz;
use mcp_server_core::observability::content_logging_enabled;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool_handler};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;

pub mod dates;
pub mod groups;
pub mod read;
pub mod write;

const INSTRUCTIONS: &str = "Tools for the iCloud Contacts of one account. \
Contacts and groups (iOS Lists) are addressed by id from search_contacts or list_groups. \
update_contact changes only the fields you pass; a list you pass (emails, phones, urls, addresses, dates, related) replaces that whole list, so include the entries to keep exactly as get_contact returned them. \
Labels are plain words such as home, work, mobile, iPhone, home page or mother. \
get_my_contact returns the owner's own card. \
upcoming_dates lists the birthdays and anniversaries coming up.";

#[derive(Clone)]
pub struct ContactsServer {
    pub(crate) store: Arc<Store>,
    /// Time zone of "today" for upcoming_dates.
    pub(crate) timezone: Tz,
    tool_router: ToolRouter<ContactsServer>,
}

impl ContactsServer {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            timezone: Tz::UTC,
            tool_router: Self::read_router()
                + Self::write_router()
                + Self::group_router()
                + Self::dates_router(),
        }
    }

    /// The time zone that decides "today" for upcoming_dates (UTC by default).
    pub fn with_timezone(mut self, timezone: Tz) -> Self {
        self.timezone = timezone;
        self
    }

    /// Names of the registered tools, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }
}

pub(crate) fn json_result(v: &Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// Runs one tool: span, metrics, and mapping of `ContactsError` to a tool error (`isError: true`)
/// whose text starts with the error kind, e.g. `conflict: ...`. Results are logged only with
/// content logging on.
pub(crate) async fn run(
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
            tracing::warn!(tool, kind = e.kind(), error = %e, "tool failed");
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{}: {e}",
                e.kind()
            ))]))
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ContactsServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(crate::config::SERVICE_NAME, crate::VERSION).with_icons(
                    mcp_server_core::service::icons(
                        include_bytes!("../../assets/icon.png"),
                        include_str!("../../assets/icon.svg"),
                    ),
                ),
            )
            .with_instructions(INSTRUCTIONS)
    }
}
