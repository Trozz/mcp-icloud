//! respond_to_invitation (registered only when ICLOUD_ALLOW_INVITES is on).

use super::{CalendarServer, json_result, run};
use crate::caldav::discovery::Account;
use crate::error::{self, CalError};
use crate::ical::build::set_my_partstat;
use crate::ical::model::PartStat;
use crate::ical::parse::master_mut;
use crate::ical::recur::{find_occurrence, override_mut};
use crate::view::{occurrence_in, occurrence_out};
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
pub struct RespondParams {
    /// event_id from list_events.
    pub event_id: String,
    /// accepted, declined or tentative.
    pub response: String,
    /// Reply for one occurrence of a recurring invitation only (occurrence_start from list_events).
    pub occurrence_start: Option<String>,
}

fn response_in(s: &str) -> error::Result<PartStat> {
    match s.trim().to_ascii_lowercase().as_str() {
        "accepted" | "accept" => Ok(PartStat::Accepted),
        "declined" | "decline" => Ok(PartStat::Declined),
        "tentative" => Ok(PartStat::Tentative),
        other => Err(CalError::invalid(format!(
            "response must be accepted, declined or tentative, not {other:?}"
        ))),
    }
}

impl CalendarServer {
    async fn respond(&self, p: RespondParams) -> error::Result<CallToolResult> {
        if !self.settings.allow_invites {
            return Err(CalError::invalid(
                "invitations are disabled on this server (ICLOUD_ALLOW_INVITES=false)",
            ));
        }
        let status = response_in(&p.response)?;
        let zone = self.settings.zones.active();
        let account = self.dir.account().await?;
        let mut e = self.load(&account, &p.event_id).await?;
        let invited = e.master.my_status().is_some();
        if e.master.i_organize() || e.master.organizer.is_none() || !invited {
            return Err(CalError::invalid(
                "this event is not an invitation to you from someone else",
            ));
        }
        let now = Utc::now();
        let mut out = serde_json::Map::new();
        match p.occurrence_start.as_deref() {
            None => {
                let m = master_mut(&mut e.ical).expect("checked in load");
                set_my_partstat(m, &account.me, &status, now)?;
            }
            Some(input) => {
                let rid = find_occurrence(&e.ical, &occurrence_in(input, zone)?, zone)?;
                out.insert("occurrence_start".into(), json!(occurrence_out(&rid, zone)));
                let o = override_mut(&mut e.ical, &rid, zone)?;
                set_my_partstat(o, &account.me, &status, now)?;
            }
        }
        self.dir
            .client()
            .put(&e.url, e.ical.to_ics(), Precondition::IfMatch(e.etag))
            .await?;
        out.insert("event_id".into(), json!(Account::event_id(&e.url)));
        out.insert("my_status".into(), json!(status.as_output()));
        Ok(json_result(&Value::Object(out)))
    }
}

#[tool_router(router = invite_router, vis = "pub(crate)")]
impl CalendarServer {
    #[tool(
        description = "Reply to an invitation from someone else: accepted, declined or tentative. iCloud sends the reply to the organizer. Pass occurrence_start to reply for one occurrence of a recurring invitation.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn respond_to_invitation(
        &self,
        Parameters(p): Parameters<RespondParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!(event_id = %p.event_id, response = %p.response, "respond_to_invitation");
        run("respond_to_invitation", self.respond(p)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_parse() {
        assert_eq!(response_in("Accepted").unwrap(), PartStat::Accepted);
        assert_eq!(response_in("decline").unwrap(), PartStat::Declined);
        assert_eq!(response_in("tentative").unwrap(), PartStat::Tentative);
        assert_eq!(response_in("maybe").unwrap_err().kind(), "validation");
    }
}
