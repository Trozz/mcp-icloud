//! create_calendar, rename_calendar and delete_calendar.

use super::write::{new_uid, writable};
use super::{CalendarServer, calendar_json, json_result, run};
use crate::caldav::discovery::Account;
use crate::caldav::xml::{mkcalendar, propertyupdate, propfind_members};
use crate::error::{self, CalError};
use mcp_icloud_dav::discovery::join;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateCalendarParams {
    /// Name of the new calendar. Must differ from every calendar's name, ignoring case.
    pub name: String,
    /// Colour as #RRGGBB, for example #1BADF8.
    pub color: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RenameCalendarParams {
    /// Calendar name or id from list_calendars.
    pub calendar: String,
    /// New name. Must not be another calendar's name, ignoring case.
    pub new_name: Option<String>,
    /// New colour as #RRGGBB, for example #1BADF8.
    pub color: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteCalendarParams {
    /// Calendar name or id from list_calendars. It must hold no events or other items.
    pub calendar: String,
}

/// A calendar name: trimmed, not empty, without control characters.
fn name_in(raw: &str, what: &str) -> error::Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(CalError::invalid(format!("{what} must not be empty")));
    }
    if name.chars().any(char::is_control) {
        return Err(CalError::invalid(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(name.to_string())
}

/// `#RRGGBB` in any case, returned as iCloud stores colours: upper case with an opaque alpha.
fn color_in(raw: &str) -> error::Result<String> {
    let hex = raw
        .trim()
        .strip_prefix('#')
        .filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()));
    let Some(hex) = hex else {
        return Err(CalError::invalid(format!(
            "color must be #RRGGBB, for example #1BADF8, not {raw:?}"
        )));
    };
    Ok(format!("#{}FF", hex.to_ascii_uppercase()))
}

fn name_free(account: &Account, name: &str, except: Option<&str>) -> error::Result<()> {
    if !account.name_taken(name, except) {
        return Ok(());
    }
    Err(CalError::Exists(format!(
        "a calendar named {name:?} already exists"
    )))
}

impl CalendarServer {
    async fn make_calendar(&self, p: CreateCalendarParams) -> error::Result<CallToolResult> {
        let name = name_in(&p.name, "name")?;
        let color = p.color.as_deref().map(color_in).transpose()?;
        let account = self.dir.refresh().await?;
        name_free(&account, &name, None)?;
        let url = account.new_calendar_url(&new_uid())?;
        self.dir
            .client()
            .mkcalendar(&url, mkcalendar(&name, color.as_deref()))
            .await?;
        let account = self.dir.refresh().await?;
        let calendar = account
            .calendars
            .iter()
            .find(|c| c.url.path() == url.path())
            .map(calendar_json)
            .unwrap_or_else(
                || json!({"id": url.path(), "name": name, "color": color, "writable": true}),
            );
        Ok(json_result(&json!({"created": true, "calendar": calendar})))
    }

    async fn patch_calendar(&self, p: RenameCalendarParams) -> error::Result<CallToolResult> {
        let new_name = p
            .new_name
            .as_deref()
            .map(|n| name_in(n, "new_name"))
            .transpose()?;
        let color = p.color.as_deref().map(color_in).transpose()?;
        if new_name.is_none() && color.is_none() {
            return Err(CalError::invalid(
                "nothing to change: give new_name, color or both",
            ));
        }
        let account = self.dir.refresh().await?;
        let cal = account.calendar(&p.calendar)?;
        writable(cal)?;
        if let Some(n) = &new_name {
            name_free(&account, n, Some(&cal.id))?;
        }
        self.dir
            .client()
            .proppatch(
                &cal.url,
                propertyupdate(new_name.as_deref(), color.as_deref()),
            )
            .await?;
        let id = cal.id.clone();
        let account = self.dir.refresh().await?;
        let updated = account
            .calendars
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                CalError::Upstream(format!(
                    "calendar {id} is no longer listed after the change"
                ))
            })?;
        let name_applied = new_name.as_ref().is_none_or(|n| &updated.name == n);
        let color_applied = color.as_ref().is_none_or(|want| {
            updated
                .color
                .as_deref()
                .and_then(|got| got.get(..7))
                .is_some_and(|got| got.eq_ignore_ascii_case(&want[..7]))
        });
        if !(name_applied && color_applied) {
            return Err(CalError::Upstream(
                "iCloud accepted the change but did not apply it".into(),
            ));
        }
        Ok(json_result(
            &json!({"updated": true, "calendar": calendar_json(updated)}),
        ))
    }

    async fn remove_calendar(&self, p: DeleteCalendarParams) -> error::Result<CallToolResult> {
        let account = self.dir.refresh().await?;
        let cal = account.calendar(&p.calendar)?;
        writable(cal)?;
        if account.calendars.iter().filter(|c| c.writable).count() < 2 {
            return Err(CalError::invalid(format!(
                "{} is the only writable calendar, so it cannot be deleted",
                cal.name
            )));
        }
        let client = self.dir.client();
        let (answered, members) = client.propfind(&cal.url, 1, propfind_members()).await?;
        let own = answered.path().trim_end_matches('/');
        let items = members
            .iter()
            .filter(|r| {
                !join(&answered, &r.href).is_ok_and(|u| u.path().trim_end_matches('/') == own)
            })
            .count();
        if items > 0 {
            let noun = if items == 1 { "item" } else { "items" };
            return Err(CalError::invalid(format!(
                "{} is not empty ({items} {noun})",
                cal.name
            )));
        }
        client.delete_collection(&cal.url).await?;
        self.dir.invalidate().await;
        Ok(json_result(
            &json!({"deleted": true, "id": cal.id, "name": cal.name}),
        ))
    }
}

#[tool_router(router = manage_router, vis = "pub(crate)")]
impl CalendarServer {
    #[tool(
        description = "Create a calendar for events with a name and an optional colour (#RRGGBB). The name must differ from every calendar's name, ignoring case. Returns the calendar as list_calendars shows it.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn create_calendar(
        &self,
        Parameters(p): Parameters<CreateCalendarParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("create_calendar");
        run("create_calendar", self.make_calendar(p)).await
    }

    #[tool(
        description = "Rename a calendar, change its colour (#RRGGBB), or both. Give new_name, color or both. The new name must not be another calendar's name, ignoring case. Read-only calendars are refused.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn rename_calendar(
        &self,
        Parameters(p): Parameters<RenameCalendarParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("rename_calendar");
        run("rename_calendar", self.patch_calendar(p)).await
    }

    #[tool(
        description = "Delete an empty calendar. Refused while it holds any event or other item, and for the only writable calendar. The emptiness check and the delete are two requests: an event added in between is deleted with the calendar.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn delete_calendar(
        &self,
        Parameters(p): Parameters<DeleteCalendarParams>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("delete_calendar");
        run("delete_calendar", self.remove_calendar(p)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_must_be_printable() {
        assert_eq!(name_in("  Garden ", "name").unwrap(), "Garden");
        for bad in ["", "   ", "a\tb", "a\nb", "a\u{7}b"] {
            assert_eq!(
                name_in(bad, "name").unwrap_err().kind(),
                "validation",
                "{bad:?}"
            );
        }
        assert!(
            name_in("", "new_name")
                .unwrap_err()
                .to_string()
                .contains("new_name")
        );
    }

    #[test]
    fn colors_are_rrggbb_sent_with_an_opaque_alpha() {
        assert_eq!(color_in("#1badf8").unwrap(), "#1BADF8FF");
        assert_eq!(color_in(" #00FF00 ").unwrap(), "#00FF00FF");
        for bad in [
            "red",
            "1BADF8",
            "#12345",
            "#1234567",
            "#1BADF8FF",
            "#GGGGGG",
            "#ééé",
        ] {
            assert_eq!(color_in(bad).unwrap_err().kind(), "validation", "{bad:?}");
        }
    }
}
