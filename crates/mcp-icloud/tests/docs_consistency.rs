//! The documentation site in `docs/src` must match the code: every environment variable the
//! server reads is in the configuration reference, every tool a service registers is on its
//! service page, and the docs name nothing the code does not have. Names are read from the code
//! at runtime, so new variables and tools fail this test until they are documented.
use mcp_icloud::{SERVICES, load};
use mcp_server_core::service::ServiceEnv;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

fn page(path: &str) -> String {
    let full = format!("{}/../../docs/src/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read {full}: {e}"))
}

/// The backticked names in the first cell of every table row of a page.
fn first_column_names(markdown: &str) -> BTreeSet<String> {
    markdown
        .lines()
        .filter_map(|line| line.strip_prefix('|'))
        .filter_map(|rest| rest.split('|').next())
        .flat_map(|cell| {
            cell.split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn is_variable(name: &str) -> bool {
    name.len() > 1
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Values that let every service read its whole configuration, with the optional tools on.
fn value(name: &str) -> Option<String> {
    let v = match name {
        "PUBLIC_URL" => "https://icloud.example.com",
        "OAUTH_SIGNING_KEY" => "0123456789abcdef0123456789abcdef",
        "OAUTH_ADMIN_PASSWORD" => "a long password",
        "INSECURE_HTTP" => "true",
        "ICLOUD_EMAIL" => "me@icloud.com",
        "ICLOUD_APP_PASSWORD" => "abcd-efgh-ijkl-mnop",
        "ICLOUD_ALLOW_SEND" => "true",
        "ICLOUD_ALLOW_INVITES" => "true",
        _ => return None,
    };
    Some(v.to_string())
}

struct Read {
    variables: BTreeSet<String>,
    tools: Vec<(&'static str, Vec<String>)>,
}

fn read_by_the_server() -> Read {
    let seen = Arc::new(Mutex::new(BTreeSet::new()));
    let record = seen.clone();
    let mut env = ServiceEnv::new(Box::new(move |k: &str| {
        record.lock().unwrap().insert(k.to_string());
        value(k)
    }));
    let loaded = load(&mut env, SERVICES).unwrap_or_else(|| panic!("{:?}", env.errors));
    let variables = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|k| !k.ends_with("_FILE"))
        .cloned()
        .collect();
    let tools = loaded
        .services
        .iter()
        .map(|s| (s.name(), s.tool_names()))
        .collect();
    Read { variables, tools }
}

// Tokio runtime: building the mail service creates its SMTP transport, which needs one.
#[tokio::test]
async fn every_variable_the_server_reads_is_documented() {
    let documented: BTreeSet<String> = first_column_names(&page("configuration.md"))
        .into_iter()
        .filter(|n| is_variable(n))
        .collect();
    let read = read_by_the_server().variables;
    let missing: Vec<&String> = read.difference(&documented).collect();
    assert!(
        missing.is_empty(),
        "read by the server but missing from docs/src/configuration.md: {missing:?}"
    );
    let stale: Vec<&String> = documented.difference(&read).collect();
    assert!(
        stale.is_empty(),
        "in docs/src/configuration.md but never read by the server: {stale:?}"
    );
}

#[tokio::test]
async fn every_registered_tool_is_on_its_service_page() {
    let read = read_by_the_server();
    assert!(!read.tools.is_empty());
    for (service, tools) in read.tools {
        let path = format!("services/{service}.md");
        let documented = first_column_names(&page(&path));
        let registered: BTreeSet<String> = tools.into_iter().collect();
        let missing: Vec<&String> = registered.difference(&documented).collect();
        assert!(
            missing.is_empty(),
            "registered by {service} but missing from docs/src/{path}: {missing:?}"
        );
        let stale: Vec<&String> = documented.difference(&registered).collect();
        assert!(
            stale.is_empty(),
            "in docs/src/{path} but not registered by {service}: {stale:?}"
        );
    }
}

#[test]
fn the_first_column_parser_reads_backticked_names() {
    let names = first_column_names(
        "| Variable | Default |\n|---|---|\n| `A_B`, `C` | `x` |\n| plain | `y` |\ntext `Z`\n",
    );
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        ["A_B".to_string(), "C".to_string()]
    );
}
