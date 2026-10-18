//! create_calendar, rename_calendar and delete_calendar.

use super::write::new_uid;
use super::{CalendarServer, calendar_json, json_result, run};
use crate::caldav::discovery::Account;
use crate::caldav::xml::mkcalendar;
use crate::error::{self, CalError};
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
