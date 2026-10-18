//! upcoming_dates.

use super::{ContactsServer, json_result, run};
use crate::dates::{self, DEFAULT_DAYS, MAX_DAYS};
use crate::error::{self, ContactsError};
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpcomingParams {
    /// How many days ahead to look, 1 to 366; today and the last day are included. Default 30.
    pub days: Option<u32>,
}

#[tool_router(router = dates_router, vis = "pub(crate)")]
impl ContactsServer {
    #[tool(
        description = "Birthdays, anniversaries and other dates of your contacts from today to the given number of days ahead (default 30, at most 366), soonest first. Each entry has the contact id and name, the label (birthday, anniversary or the date's own label), the date, the days from today and, when the year is known, the age or number of years reached.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn upcoming_dates(
        &self,
        Parameters(p): Parameters<UpcomingParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(days = ?p.days, "upcoming_dates");
        run("upcoming_dates", self.upcoming(p)).await
    }
}

impl ContactsServer {
    async fn upcoming(&self, p: UpcomingParams) -> error::Result<CallToolResult> {
        let days = p.days.unwrap_or(DEFAULT_DAYS);
        if !(1..=MAX_DAYS).contains(&days) {
            return Err(ContactsError::invalid(format!(
                "days must be between 1 and {MAX_DAYS}"
            )));
        }
        let today = Utc::now().with_timezone(&self.timezone).date_naive();
        let idx = self.store.open().await?;
        let found = dates::upcoming(&idx.contacts(), today, days);
        Ok(json_result(&json!({
            "today": today.format("%Y-%m-%d").to_string(),
            "days": days,
            "dates": found,
        })))
    }
}
