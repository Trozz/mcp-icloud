//! The mail service's MCP prompts. Texts name the tools with the endpoint's tool prefix: empty
//! on `/mail`, `mail_` under `/mcp`.

use mcp_server_core::prompt::{UNAVAILABLE, argument, not_found, number_argument, user_prompt};
use rmcp::ErrorData as McpError;
use rmcp::model::{GetPromptResult, JsonObject, Prompt, PromptArgument};
use serde_json::Value;

const TRIAGE_INBOX: &str = "triage_inbox";
const TRIAGE_DESCRIPTION: &str = "Go through unread mail: summarise each message and suggest replying, flagging, moving or deleting it. Writes reply drafts only when asked and never sends.";

/// Every prompt of the mail service.
pub fn list() -> Vec<Prompt> {
    vec![
        Prompt::new(
            TRIAGE_INBOX,
            Some(TRIAGE_DESCRIPTION),
            Some(vec![
                PromptArgument::new("mailbox")
                    .with_description("Mailbox to triage. Default INBOX.")
                    .with_required(false),
                PromptArgument::new("limit")
                    .with_description("How many unread messages to look at, 1 to 100. Default 25.")
                    .with_required(false),
            ]),
        )
        .with_title("Triage inbox"),
    ]
}

/// Renders prompt `name`, naming tools as `<tool_prefix><tool>`.
pub fn get(
    name: &str,
    arguments: Option<&JsonObject>,
    tool_prefix: &str,
) -> Result<GetPromptResult, McpError> {
    match name {
        TRIAGE_INBOX => triage_inbox(arguments, tool_prefix),
        _ => Err(not_found(name)),
    }
}

fn triage_inbox(arguments: Option<&JsonObject>, p: &str) -> Result<GetPromptResult, McpError> {
    let mailbox = argument(arguments, "mailbox")?.unwrap_or_else(|| "INBOX".to_string());
    let limit = number_argument(arguments, "limit", 25, 1..=100)?;
    let mailbox = Value::String(mailbox).to_string();
    let text = format!(
        "Triage the unread mail in the mailbox {mailbox}.\n\n\
1. Call `{p}search_messages` with {{\"mailbox\": {mailbox}, \"unseen\": true, \"limit\": {limit}}}. If it finds nothing, say there is no unread mail there and stop.\n\
2. For each message give the sender, the subject and a one-line summary. Call `{p}get_message` only when the summary is not enough to judge a message; reading it does not mark it as read.\n\
3. Suggest one action per message: reply, flag, move (name the target mailbox) or delete. Ask me which suggestions to carry out.\n\
4. Carry out only the actions I approve, passing the uidvalidity from step 1: `{p}mark_messages` to flag or mark as read, `{p}move_messages` to move, `{p}delete_messages` to move to Trash.\n\
5. Write a reply only when I ask for one: show me the text first, then save it with `{p}create_reply_draft`.\n\n\
Never send a message: replies stay in Drafts for me to review.\n\
{UNAVAILABLE}"
    );
    Ok(user_prompt(TRIAGE_DESCRIPTION, text))
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
    fn the_mail_service_offers_triage_inbox() {
        let prompts = list();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "triage_inbox");
        let names: Vec<&str> = prompts[0]
            .arguments
            .as_ref()
            .unwrap()
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["mailbox", "limit"]);
        assert!(
            prompts[0]
                .arguments
                .as_ref()
                .unwrap()
                .iter()
                .all(|a| a.required == Some(false))
        );
    }

    #[test]
    fn triage_names_only_existing_mail_tools() {
        let t = text("triage_inbox", None, "");
        assert_eq!(
            backticked(&t),
            [
                "search_messages",
                "get_message",
                "mark_messages",
                "move_messages",
                "delete_messages",
                "create_reply_draft"
            ]
        );
        assert!(t.contains("Never send a message"), "{t}");
        assert!(t.ends_with(UNAVAILABLE), "{t}");
    }

    #[test]
    fn defaults_are_inbox_and_25() {
        let t = text("triage_inbox", None, "");
        assert!(
            t.contains(
                r#"`search_messages` with {"mailbox": "INBOX", "unseen": true, "limit": 25}"#
            ),
            "{t}"
        );
    }

    #[test]
    fn blank_arguments_use_inbox_and_25() {
        let a = args(json!({"mailbox": " ", "limit": ""}));
        assert_eq!(
            text("triage_inbox", Some(&a), ""),
            text("triage_inbox", None, "")
        );
    }

    #[test]
    fn arguments_are_used() {
        let a = args(json!({"mailbox": "Work", "limit": "10"}));
        let t = text("triage_inbox", Some(&a), "");
        assert!(
            t.contains(r#"{"mailbox": "Work", "unseen": true, "limit": 10}"#),
            "{t}"
        );
        assert!(
            t.starts_with(r#"Triage the unread mail in the mailbox "Work"."#),
            "{t}"
        );
    }

    #[test]
    fn mailbox_names_are_json_escaped() {
        let a = args(json!({"mailbox": "Say \"hi\"\\x"}));
        let t = text("triage_inbox", Some(&a), "");
        assert!(t.contains(r#"{"mailbox": "Say \"hi\"\\x", "#), "{t}");
    }

    #[test]
    fn the_prefix_goes_on_every_tool_name_and_nowhere_else() {
        let a = args(json!({"limit": "5"}));
        assert_eq!(
            text("triage_inbox", Some(&a), "mail_"),
            with_prefix(&text("triage_inbox", Some(&a), ""), "mail_")
        );
    }

    #[test]
    fn invalid_arguments_are_invalid_params() {
        for a in [
            json!({"limit": "0"}),
            json!({"limit": "101"}),
            json!({"limit": "ten"}),
            json!({"limit": 25}),
            json!({"mailbox": 3}),
        ] {
            let e = get("triage_inbox", Some(&args(a.clone())), "").unwrap_err();
            assert_eq!(e.code, ErrorCode::INVALID_PARAMS, "{a}");
        }
    }

    #[test]
    fn unknown_prompts_are_not_found() {
        let e = get("plan_week", None, "").unwrap_err();
        assert_eq!(e.message, "prompt 'plan_week' not found");
    }
}
