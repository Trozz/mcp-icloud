use super::fake_caldav::{EMAIL, FakeCalDav, PASSWORD};
use mcp_icloud_calendar::caldav::caldav_settings;
use mcp_icloud_calendar::caldav::discovery::Directory;
use mcp_icloud_calendar::tools::{CalendarServer, ToolSettings};
use mcp_icloud_calendar::tz::ZoneState;
use mcp_icloud_dav::client::DavClient;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub fn directory(fake: &FakeCalDav) -> Arc<Directory> {
    let client = DavClient::new(caldav_settings(
        fake.url.clone(),
        EMAIL.into(),
        SecretString::from(PASSWORD),
    ))
    .unwrap();
    Arc::new(Directory::new(Arc::new(client), EMAIL.into()))
}

pub fn settings(allow_invites: bool) -> Arc<ToolSettings> {
    Arc::new(ToolSettings {
        zones: ZoneState::new(chrono_tz::Europe::London),
        allow_invites,
        email: EMAIL.into(),
    })
}

pub fn server(fake: &FakeCalDav, allow_invites: bool) -> CalendarServer {
    CalendarServer::new(directory(fake), settings(allow_invites))
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

/// The error text of a failed tool call, failing the test when the call succeeded.
pub fn error_text(r: &CallToolResult) -> String {
    assert!(is_error(r), "expected a tool error, got: {}", text(r));
    text(r)
}
