use super::fake_imap::FakeImap;
use mcp_icloud_mail::cache::LruMessageCache;
use mcp_icloud_mail::imap::{Connector, ImapClient, ImapSettings};
use mcp_icloud_mail::mail::build::Identity;
use mcp_icloud_mail::smtp::Sender;
use mcp_icloud_mail::tools::{MailServer, ToolSettings};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;

pub const ME: &str = "me@icloud.com";

pub fn imap(fake: &FakeImap) -> Arc<ImapClient> {
    Arc::new(ImapClient::new(
        ImapSettings {
            connector: Connector::Plain {
                addr: fake.addr.clone(),
            },
            username: ME.into(),
            password: "app-pass".to_string().into(),
            idle_timeout: Duration::from_secs(600),
        },
        Arc::new(LruMessageCache::new(1024 * 1024)),
    ))
}

pub fn settings(max_attachment_bytes: usize) -> Arc<ToolSettings> {
    Arc::new(ToolSettings {
        identity: Identity {
            email: ME.into(),
            name: Some("Me Example".into()),
        },
        max_attachment_bytes,
    })
}

pub fn server(fake: &FakeImap, sender: Option<Arc<dyn Sender>>) -> MailServer {
    MailServer::new(imap(fake), sender, settings(1024 * 1024))
}

pub fn params<T: DeserializeOwned>(v: serde_json::Value) -> Parameters<T> {
    Parameters(serde_json::from_value(v).expect("valid tool arguments"))
}

pub fn is_error(r: &CallToolResult) -> bool {
    r.is_error == Some(true)
}

pub fn text(r: &CallToolResult) -> String {
    r.content[0]
        .as_text()
        .expect("first content block is text")
        .text
        .clone()
}

/// Parses the JSON text of a successful tool result, failing the test on a tool error.
pub fn json(r: &CallToolResult) -> serde_json::Value {
    assert!(!is_error(r), "tool error: {}", text(r));
    serde_json::from_str(&text(r)).expect("tool result is JSON")
}
