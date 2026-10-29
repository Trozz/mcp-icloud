//! The documentation site in `docs/src` must match the code: every environment variable the
//! server reads is in the configuration reference, every tool a service registers is on its
//! service page, and the docs name nothing the code does not have. Names are read from the code
//! at runtime, so new variables and tools fail this test until they are documented.
use mcp_icloud::server;
use mcp_server_core::service::ServiceEnv;
use rmcp::ServerHandler;
use rmcp::model::{Prompt, Tool};
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

/// The lines of a page under `heading` (a `## ` line), up to the next `## ` heading.
fn section(markdown: &str, heading: &str) -> String {
    let mut inside = false;
    markdown
        .lines()
        .filter(|line| {
            if line.starts_with("## ") {
                inside = *line == heading;
                return false;
            }
            inside
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every service's prompt names, in `SERVICES` order. A new service adds its line here.
fn registered_prompts() -> Vec<(&'static str, Vec<String>)> {
    let names =
        |prompts: Vec<Prompt>| -> Vec<String> { prompts.into_iter().map(|p| p.name).collect() };
    vec![
        ("mail", names(mcp_icloud_mail::prompts::list())),
        ("calendar", names(mcp_icloud_calendar::prompts::list())),
        ("contacts", names(mcp_icloud_contacts::prompts::list())),
    ]
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
    schemas: Vec<(&'static str, Vec<Tool>)>,
}

fn read_by_the_server() -> Read {
    let seen = Arc::new(Mutex::new(BTreeSet::new()));
    let record = seen.clone();
    let mut env = ServiceEnv::new(Box::new(move |k: &str| {
        record.lock().unwrap().insert(k.to_string());
        value(k)
    }));
    let loaded = server()
        .load(&mut env)
        .unwrap_or_else(|| panic!("{:?}", env.errors));
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
    let schemas = loaded
        .services
        .iter()
        .map(|s| {
            let handler = s.handler();
            let tools = s
                .tool_names()
                .iter()
                .filter_map(|name| handler.get_tool(name))
                .collect();
            (s.name(), tools)
        })
        .collect();
    Read {
        variables,
        tools,
        schemas,
    }
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
        let documented = first_column_names(&section(&page(&path), "## Tools"));
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

#[test]
fn the_section_parser_stops_at_the_next_heading() {
    let md = "# T\n## Tools\n| `a` |\n## Prompts\n| `b` |\n### Sub\n| `c` |\n## Notes\n| `d` |\n";
    assert_eq!(section(md, "## Tools"), "| `a` |");
    assert_eq!(section(md, "## Prompts"), "| `b` |\n### Sub\n| `c` |");
    assert_eq!(section(md, "## Missing"), "");
}

#[tokio::test]
async fn every_prompt_is_on_its_service_page_and_on_the_combined_page() {
    let services: Vec<&str> = read_by_the_server().tools.iter().map(|(s, _)| *s).collect();
    let prompts = registered_prompts();
    assert_eq!(
        prompts.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        services,
        "registered_prompts must name every service, in SERVICES order"
    );
    let mut prefixed = BTreeSet::new();
    for (service, names) in prompts {
        let path = format!("services/{service}.md");
        let documented = first_column_names(&section(&page(&path), "## Prompts"));
        let registered: BTreeSet<String> = names.iter().cloned().collect();
        assert_eq!(
            documented, registered,
            "prompts of {service} versus the Prompts table of docs/src/{path}"
        );
        prefixed.extend(names.iter().map(|n| format!("{service}_{n}")));
    }
    let combined = first_column_names(&section(&page("services/combined.md"), "## Prompts"));
    assert_eq!(
        combined, prefixed,
        "prefixed prompts versus the Prompts table of docs/src/services/combined.md"
    );
}

/// The parameters a "Key parameters" cell names: the backticked names outside parentheses, each
/// with the parenthesised text that follows it. Names of other tools are references, not parameters.
fn documented_parameters(cell: &str, tools: &BTreeSet<String>) -> Vec<(String, String)> {
    let chars: Vec<char> = cell.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let end = chars[i + 1..]
            .iter()
            .position(|c| *c == '`')
            .map_or(chars.len(), |p| i + 1 + p);
        let name: String = chars[i + 1..end].iter().collect();
        i = end + 1;
        let mut j = i;
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        let mut note = String::new();
        if j < chars.len() && chars[j] == '(' {
            let mut depth = 0;
            while j < chars.len() {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    c => note.push(c),
                }
                j += 1;
                if depth == 0 {
                    break;
                }
            }
            i = j;
        }
        if !tools.contains(&name) {
            found.push((name, note));
        }
    }
    found
}

/// The text after "default" in a note, up to the first comma or the end, without backticks.
fn documented_default(note: &str) -> Option<String> {
    let lower = note.to_lowercase();
    let after = &lower[lower.find("default ")? + "default ".len()..];
    let value = after.split(',').next()?.replace('`', "");
    Some(value.trim().to_string())
}

fn rows(markdown: &str) -> Vec<Vec<String>> {
    markdown
        .lines()
        .filter(|l| l.starts_with('|'))
        .map(|l| l.split('|').map(|c| c.trim().to_string()).collect())
        .collect()
}

#[tokio::test]
async fn documented_parameters_and_defaults_match_the_tool_schemas() {
    let read = read_by_the_server();
    let mut problems = Vec::new();
    for (service, tools) in &read.schemas {
        let markdown = page(&format!("services/{service}.md"));
        let rows = rows(&markdown);
        let names: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();
        for tool in tools {
            let name = tool.name.to_string();
            let Some(row) = rows
                .iter()
                .find(|r| r.len() == 6 && r[1] == format!("`{name}`"))
            else {
                problems.push(format!("{service}/{name}: no table row"));
                continue;
            };
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            let required: BTreeSet<String> = tool
                .input_schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|r| {
                    r.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let documented = documented_parameters(&row[4], &names);
            let heads: BTreeSet<&String> = documented.iter().map(|(n, _)| n).collect();
            for (param, note) in &documented {
                let Some(schema) = properties.get(param) else {
                    problems.push(format!(
                        "{service}/{name}: documents unknown parameter {param}"
                    ));
                    continue;
                };
                let Some(default) = documented_default(note) else {
                    continue;
                };
                let description = schema
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default()
                    .to_lowercase();
                if !description.contains("default") || !description.contains(&default) {
                    problems.push(format!(
                        "{service}/{name}: {param} documented default {default:?}, schema says {description:?}"
                    ));
                }
            }
            for param in &required {
                if !heads.contains(param) {
                    problems.push(format!(
                        "{service}/{name}: required parameter {param} not documented"
                    ));
                }
            }
            for param in properties.keys() {
                let optional_and_hidden = !required.contains(param) && !heads.contains(param);
                if optional_and_hidden
                    && !row[4].contains("fields")
                    && !row[4].contains("any field")
                {
                    problems.push(format!(
                        "{service}/{name}: parameter {param} not documented"
                    ));
                }
            }
            for (param, note) in &documented {
                if documented_default(note).is_none()
                    && required.contains(param)
                    && note.to_lowercase().contains("optional")
                {
                    problems.push(format!(
                        "{service}/{name}: {param} is required, documented optional"
                    ));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_parameter_parser_reads_names_notes_and_defaults() {
    let tools = BTreeSet::from(["other".to_string()]);
    let found = documented_parameters(
        "`a`, `limit` (1 to 100, default 25), `b` and `c` (`x`, `y`), see `other`",
        &tools,
    );
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a", "limit", "b", "c"]);
    assert_eq!(documented_default(&found[1].1).as_deref(), Some("25"));
    assert_eq!(documented_default(&found[0].1), None);
}

/// Every name the source passes to an environment reader, outside the unit tests. A variable read
/// only when another is set would be missing from the recorded set, and this still finds it.
fn names_in_source() -> BTreeSet<String> {
    const READERS: [&str; 8] = [
        "required",
        "secret_required",
        "optional",
        "secret_optional",
        "bool",
        "parse",
        "parse_optional",
        "list",
    ];
    let crates = format!("{}/..", env!("CARGO_MANIFEST_DIR"));
    let mut names = BTreeSet::new();
    let mut dirs = vec![std::path::PathBuf::from(&crates)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !path.ends_with("target") && !path.ends_with("tests") {
                    dirs.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap();
            let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
            for reader in READERS {
                let call = format!("env.{reader}(\"");
                let call = call.as_str();
                for part in squeezed.split(call).skip(1) {
                    if let Some(name) = part.split('"').next().filter(|n| is_variable(n)) {
                        names.insert(name.to_string());
                    }
                }
                let chained = format!(".{reader}(\"");
                for part in squeezed.split(chained.as_str()).skip(1) {
                    if let Some(name) = part.split('"').next().filter(|n| is_variable(n)) {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names
}

#[tokio::test]
async fn no_variable_in_the_source_escapes_the_recorded_reads() {
    let read = read_by_the_server().variables;
    // The server's own variables (PUBLIC_URL, OAUTH_*, ICLOUD_SERVICES, ...) are read by
    // mcp-server-core, outside this repository, every time it loads; these are the services'.
    let source = names_in_source();
    assert!(
        source.contains("ICLOUD_EMAIL") && source.contains("CARDDAV_URL"),
        "{source:?}"
    );
    assert!(read.contains("PUBLIC_URL") && read.contains("ICLOUD_READ_ONLY"));
    let unseen: Vec<&String> = source.difference(&read).collect();
    assert!(
        unseen.is_empty(),
        "named in the source but not read when the harness loads the server (read only under a \
         condition? supply the value in `value`): {unseen:?}"
    );
}
