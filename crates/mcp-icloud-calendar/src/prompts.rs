//! The calendar service's MCP prompts. Texts name the tools with the endpoint's tool prefix:
//! empty on `/calendar`, `calendar_` under `/mcp`.

use chrono::{Duration, NaiveDate};
use mcp_server_core::prompt::{UNAVAILABLE, argument, not_found, user_prompt};
use rmcp::ErrorData as McpError;
use rmcp::model::{GetPromptResult, JsonObject, Prompt, PromptArgument};

const PLAN_WEEK: &str = "plan_week";
const PLAN_WEEK_DESCRIPTION: &str = "Plan a week: lay out the events, point out conflicts and invitations waiting for a reply, and suggest free time.";

/// Every prompt of the calendar service.
pub fn list() -> Vec<Prompt> {
    vec![
        Prompt::new(
            PLAN_WEEK,
            Some(PLAN_WEEK_DESCRIPTION),
            Some(vec![
                PromptArgument::new("start")
                    .with_description(
                        "First day of the week, YYYY-MM-DD. Default today in the active time zone.",
                    )
                    .with_required(false),
            ]),
        )
        .with_title("Plan the week"),
    ]
}

/// Renders prompt `name`, naming tools as `<tool_prefix><tool>`. `today` is the date in the
/// active time zone, the default start.
pub fn get(
    name: &str,
    arguments: Option<&JsonObject>,
    tool_prefix: &str,
    today: NaiveDate,
) -> Result<GetPromptResult, McpError> {
    match name {
        PLAN_WEEK => plan_week(arguments, tool_prefix, today),
        _ => Err(not_found(name)),
    }
}

fn start_date(arguments: Option<&JsonObject>, today: NaiveDate) -> Result<NaiveDate, McpError> {
    let Some(text) = argument(arguments, "start")? else {
        return Ok(today);
    };
    let invalid = || McpError::invalid_params("start must be a date as YYYY-MM-DD", None);
    if text.len() != 10 {
        return Err(invalid());
    }
    NaiveDate::parse_from_str(&text, "%Y-%m-%d").map_err(|_| invalid())
}

fn plan_week(
    arguments: Option<&JsonObject>,
    p: &str,
    today: NaiveDate,
) -> Result<GetPromptResult, McpError> {
    let start = start_date(arguments, today)?;
    let end = start + Duration::days(7);
    let last = end - Duration::days(1);
    let text = format!(
        "Help me plan the week from {start} to {last}.\n\n\
1. Call `{p}list_events` with {{\"start\": \"{start}\", \"end\": \"{end}\"}}. Lay the week out day by day: time, title and calendar of each event.\n\
2. Point out conflicts: events that overlap, and days with back-to-back events and no break.\n\
3. Call `{p}list_events` with {{\"start\": \"{start}\", \"end\": \"{end}\", \"needs_response\": true}} and list the invitations still waiting for my reply. Ask me whether to accept, decline or accept tentatively, and call `{p}respond_to_invitation` only with the answer I give for each one.\n\
4. Call `{p}find_free_time` with {{\"start\": \"{start}\", \"end\": \"{end}\", \"duration_minutes\": 60, \"working_hours\": {{\"start\": \"09:00\", \"end\": \"17:00\"}}}} and suggest the best free slots for focused work or for moving a conflicting event.\n\n\
Do not create, change or delete events unless I ask.\n\
{UNAVAILABLE}"
    );
    Ok(user_prompt(PLAN_WEEK_DESCRIPTION, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;
    use serde_json::json;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()
    }

    fn args(v: serde_json::Value) -> JsonObject {
        v.as_object().unwrap().clone()
    }

    fn text(a: Option<&JsonObject>, prefix: &str) -> String {
        let r = get("plan_week", a, prefix, today()).unwrap();
        r.messages[0].content.as_text().unwrap().text.clone()
    }

    fn backticked(text: &str) -> Vec<&str> {
        text.split('`').skip(1).step_by(2).collect()
    }

    fn with_prefix(text: &str, prefix: &str) -> String {
        text.split('`')
            .enumerate()
            .map(|(i, part)| {
                if i % 2 == 1 {
                    format!("{prefix}{part}")
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("`")
    }

    #[test]
    fn the_calendar_service_offers_plan_week() {
        let prompts = list();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "plan_week");
        let arguments = prompts[0].arguments.as_ref().unwrap();
        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].name, "start");
        assert_eq!(arguments[0].required, Some(false));
    }

    #[test]
    fn plan_week_names_only_existing_calendar_tools() {
        let t = text(None, "");
        assert_eq!(
            backticked(&t),
            [
                "list_events",
                "list_events",
                "respond_to_invitation",
                "find_free_time"
            ]
        );
        assert!(t.contains("only with the answer I give"), "{t}");
        assert!(t.ends_with(UNAVAILABLE), "{t}");
    }

    #[test]
    fn the_week_starts_today_by_default() {
        let t = text(None, "");
        assert!(
            t.starts_with("Help me plan the week from 2026-10-19 to 2026-10-25."),
            "{t}"
        );
        assert!(
            t.contains(r#"`list_events` with {"start": "2026-10-19", "end": "2026-10-26"}"#),
            "{t}"
        );
        assert!(
            t.contains(r#"{"start": "2026-10-19", "end": "2026-10-26", "needs_response": true}"#),
            "{t}"
        );
        assert!(
            t.contains(r#"`find_free_time` with {"start": "2026-10-19", "end": "2026-10-26", "duration_minutes": 60"#),
            "{t}"
        );
        let blank = args(json!({"start": ""}));
        assert_eq!(text(Some(&blank), ""), t);
    }

    #[test]
    fn a_given_start_may_cross_the_year() {
        let a = args(json!({"start": "2026-12-29"}));
        let t = text(Some(&a), "");
        assert!(
            t.starts_with("Help me plan the week from 2026-12-29 to 2027-01-04."),
            "{t}"
        );
        assert!(t.contains(r#""end": "2027-01-05""#), "{t}");
    }

    #[test]
    fn the_prefix_goes_on_every_tool_name_and_nowhere_else() {
        assert_eq!(
            text(None, "calendar_"),
            with_prefix(&text(None, ""), "calendar_")
        );
    }

    #[test]
    fn start_must_be_a_real_yyyy_mm_dd_date() {
        for bad in [
            json!({"start": "2026-2-03"}),
            json!({"start": "2026-13-01"}),
            json!({"start": "2026-02-30"}),
            json!({"start": "19 October"}),
            json!({"start": "2026-10-19T09:00"}),
            json!({"start": 20261019}),
        ] {
            let e = get("plan_week", Some(&args(bad.clone())), "", today()).unwrap_err();
            assert_eq!(e.code, ErrorCode::INVALID_PARAMS, "{bad}");
        }
        let e = get(
            "plan_week",
            Some(&args(json!({"start": "2026-13-01"}))),
            "",
            today(),
        )
        .unwrap_err();
        assert_eq!(e.message, "start must be a date as YYYY-MM-DD");
    }

    #[test]
    fn unknown_prompts_are_not_found() {
        let e = get("triage_inbox", None, "", today()).unwrap_err();
        assert_eq!(e.message, "prompt 'triage_inbox' not found");
    }
}
