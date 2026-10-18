//! The contacts service's MCP prompts. Texts name the tools with the endpoint's tool prefix:
//! empty on `/contacts`, `contacts_` under `/mcp`.

use mcp_server_core::prompt::{UNAVAILABLE, not_found, number_argument, user_prompt};
use rmcp::ErrorData as McpError;
use rmcp::model::{GetPromptResult, JsonObject, Prompt, PromptArgument};

const BIRTHDAY_REMINDERS: &str = "birthday_reminders";
const BIRTHDAY_DESCRIPTION: &str = "List the birthdays and anniversaries coming up, by date, and offer to write messages. Never sends.";
const CLEAN_UP_DUPLICATES: &str = "clean_up_duplicates";
const CLEAN_UP_DESCRIPTION: &str = "Find duplicate contacts and merge them one group at a time, only after you confirm each group.";

/// Every prompt of the contacts service.
pub fn list() -> Vec<Prompt> {
    vec![
        Prompt::new(
            BIRTHDAY_REMINDERS,
            Some(BIRTHDAY_DESCRIPTION),
            Some(vec![
                PromptArgument::new("days")
                    .with_description("How many days ahead to look, 1 to 366. Default 14.")
                    .with_required(false),
            ]),
        )
        .with_title("Birthday reminders"),
        Prompt::new(CLEAN_UP_DUPLICATES, Some(CLEAN_UP_DESCRIPTION), None)
            .with_title("Clean up duplicates"),
    ]
}

/// Renders prompt `name`, naming tools as `<tool_prefix><tool>`.
pub fn get(
    name: &str,
    arguments: Option<&JsonObject>,
    tool_prefix: &str,
) -> Result<GetPromptResult, McpError> {
    match name {
        BIRTHDAY_REMINDERS => birthday_reminders(arguments, tool_prefix),
        CLEAN_UP_DUPLICATES => Ok(clean_up_duplicates(tool_prefix)),
        _ => Err(not_found(name)),
    }
}

fn birthday_reminders(
    arguments: Option<&JsonObject>,
    p: &str,
) -> Result<GetPromptResult, McpError> {
    let days = number_argument(arguments, "days", 14, 1..=366)?;
    let text = format!(
        "Remind me of my contacts' birthdays and anniversaries in the next {days} days.\n\n\
1. Call `{p}upcoming_dates` with {{\"days\": {days}}}. If it finds nothing, say nothing is coming up and stop.\n\
2. List the dates soonest first, grouped by date: the contact's name, what the date is (birthday, anniversary or its own label), how many days away it is and, when known, the age or number of years.\n\
3. Offer to write a short message for any of them. If mail drafting tools are available to you (on a combined server their names start with mail_), offer to save the message as a draft, using `{p}get_contact` for the email address; otherwise show the message here for me to copy. Never send anything.\n\n\
{UNAVAILABLE}"
    );
    Ok(user_prompt(BIRTHDAY_DESCRIPTION, text))
}

fn clean_up_duplicates(p: &str) -> GetPromptResult {
    let text = format!(
        "Help me clean up duplicate contacts.\n\n\
1. Call `{p}find_duplicate_contacts`. If it finds no groups, say so and stop.\n\
2. Go through the groups one at a time. For each group show the contacts' names and the reasons they matched, say which contact would be kept (the first one, unless I choose another) and what merging does: emails, phones, URLs, addresses, dates and related names the kept contact lacks are added to it, notes are combined, group memberships move to it, and the other contacts are deleted. Photos are not copied (`{p}copy_contact_photo` can copy one first), and social profiles, instant messaging, categories and custom fields of the merged contacts are not kept.\n\
3. Ask me to confirm that group. Call `{p}merge_contacts` with keep_id set to the kept contact and merge_ids set to the others only after I explicitly confirm that group; a yes for one group never covers another. If I decline or skip a group, leave it unchanged.\n\
4. After each merge, report the result, including photo_not_kept and anything that was not done.\n\n\
{UNAVAILABLE}"
    );
    user_prompt(CLEAN_UP_DESCRIPTION, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;
    use serde_json::json;

    fn args(v: serde_json::Value) -> JsonObject {
        v.as_object().unwrap().clone()
    }

    fn text(name: &str, a: Option<&JsonObject>, prefix: &str) -> String {
        let r = get(name, a, prefix).unwrap();
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
    fn the_contacts_service_offers_two_prompts() {
        let prompts = list();
        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["birthday_reminders", "clean_up_duplicates"]);
        let days = prompts[0].arguments.as_ref().unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].name, "days");
        assert_eq!(days[0].required, Some(false));
        assert!(prompts[1].arguments.is_none());
    }

    #[test]
    fn birthday_reminders_look_14_days_ahead_by_default() {
        let t = text("birthday_reminders", None, "");
        assert!(t.contains(r#"`upcoming_dates` with {"days": 14}"#), "{t}");
        assert!(
            t.starts_with(
                "Remind me of my contacts' birthdays and anniversaries in the next 14 days."
            ),
            "{t}"
        );
        let blank = args(json!({"days": " "}));
        assert_eq!(text("birthday_reminders", Some(&blank), ""), t);
        let a = args(json!({"days": "366"}));
        assert!(text("birthday_reminders", Some(&a), "").contains(r#"{"days": 366}"#));
    }

    #[test]
    fn birthday_reminders_name_only_contacts_tools_and_never_send() {
        let t = text("birthday_reminders", None, "");
        assert_eq!(backticked(&t), ["upcoming_dates", "get_contact"]);
        assert!(t.contains("their names start with mail_"), "{t}");
        assert!(t.contains("Never send anything."), "{t}");
        assert!(t.ends_with(UNAVAILABLE), "{t}");
    }

    #[test]
    fn days_must_be_1_to_366() {
        for bad in [
            json!({"days": "0"}),
            json!({"days": "367"}),
            json!({"days": "two"}),
            json!({"days": 7}),
        ] {
            let e = get("birthday_reminders", Some(&args(bad.clone())), "").unwrap_err();
            assert_eq!(e.code, ErrorCode::INVALID_PARAMS, "{bad}");
        }
    }

    #[test]
    fn clean_up_merges_only_after_confirmation_per_group() {
        let t = text("clean_up_duplicates", None, "");
        assert_eq!(
            backticked(&t),
            [
                "find_duplicate_contacts",
                "copy_contact_photo",
                "merge_contacts"
            ]
        );
        assert!(
            t.contains("only after I explicitly confirm that group"),
            "{t}"
        );
        assert!(
            t.contains("a yes for one group never covers another"),
            "{t}"
        );
        assert!(t.ends_with(UNAVAILABLE), "{t}");
    }

    #[test]
    fn clean_up_ignores_arguments() {
        let a = args(json!({"days": "x", "anything": 1}));
        assert_eq!(
            text("clean_up_duplicates", Some(&a), ""),
            text("clean_up_duplicates", None, "")
        );
    }

    #[test]
    fn the_prefix_goes_on_every_tool_name_and_nowhere_else() {
        for name in ["birthday_reminders", "clean_up_duplicates"] {
            assert_eq!(
                text(name, None, "contacts_"),
                with_prefix(&text(name, None, ""), "contacts_"),
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_prompts_are_not_found() {
        let e = get("plan_week", None, "").unwrap_err();
        assert_eq!(e.message, "prompt 'plan_week' not found");
    }
}
