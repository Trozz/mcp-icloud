use super::fake_carddav::FakeCardDav;
use mcp_icloud_contacts::index::Store;
use mcp_icloud_contacts::tools::ContactsServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub fn server(fake: &FakeCardDav) -> ContactsServer {
    ContactsServer::new(Arc::new(Store::new(fake.client())))
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
