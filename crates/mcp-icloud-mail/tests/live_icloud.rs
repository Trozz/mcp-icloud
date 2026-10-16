//! Live tests against a real iCloud account. Ignored by default; they need:
//!   ICLOUD_EMAIL, ICLOUD_APP_PASSWORD and optionally ICLOUD_USERNAME.
//! Run: cargo test -p mcp-icloud-mail --test live_icloud -- --ignored --nocapture --test-threads=1
//! They read the mailbox and create one draft, which they delete again (Drafts -> Trash -> expunged
//! when UIDPLUS is available). Nothing is sent.
mod support;
use futures::TryStreamExt;
use mcp_icloud_mail::cache::LruMessageCache;
use mcp_icloud_mail::imap::{ImapClient, ImapSettings, Role, SearchQuery};
use mcp_icloud_mail::mail::build::Identity;
use mcp_icloud_mail::tools::{MailServer, ToolSettings};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use support::tools::{json, params};

struct Account {
    email: String,
    username: String,
    password: String,
}

fn account() -> Account {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let email = get("ICLOUD_EMAIL").expect("set ICLOUD_EMAIL to run the live tests");
    let password =
        get("ICLOUD_APP_PASSWORD").expect("set ICLOUD_APP_PASSWORD to run the live tests");
    Account {
        username: get("ICLOUD_USERNAME").unwrap_or_else(|| email.clone()),
        email,
        password,
    }
}

fn client(a: &Account) -> Arc<ImapClient> {
    mcp_server_core::install_crypto_provider();
    Arc::new(ImapClient::new(
        ImapSettings::icloud(
            a.username.clone(),
            a.password.clone().into(),
            Duration::from_secs(600),
        ),
        Arc::new(LruMessageCache::new(16 * 1024 * 1024)),
    ))
}

/// Prints the raw post-login CAPABILITY line and every LIST entry with its attributes, so the
/// plan's assumptions (MOVE, UIDPLUS, SPECIAL-USE, folder names) can be checked once by hand.
#[tokio::test]
#[ignore]
async fn live_dump_capabilities_and_mailboxes() {
    let a = account();
    mcp_server_core::install_crypto_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(("imap.mail.me.com", 993))
        .await
        .unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(
            rustls::pki_types::ServerName::try_from("imap.mail.me.com").unwrap(),
            tcp,
        )
        .await
        .unwrap();
    let mut c = async_imap::Client::new(tls);
    c.read_response().await.unwrap();
    let mut s = c
        .login(&a.username, &a.password)
        .await
        .map_err(|(e, _)| e)
        .unwrap();
    let caps = s.capabilities().await.unwrap();
    let mut names: Vec<String> = caps.iter().map(|c| format!("{c:?}")).collect();
    names.sort();
    println!("CAPABILITY: {}", names.join(" "));
    let list: Vec<_> = s
        .list(Some(""), Some("*"))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    for n in &list {
        println!("LIST {:?} {:?}", n.name(), n.attributes());
    }
    s.logout().await.unwrap();

    let client = client(&a);
    let caps = client.capabilities().await.unwrap();
    println!("parsed: MOVE={} UIDPLUS={}", caps.move_, caps.uidplus);
    for role in [Role::Sent, Role::Drafts, Role::Trash] {
        println!(
            "{role:?} -> {:?}",
            client.role_mailbox(role).await.map_err(|e| e.to_string())
        );
    }
}

#[tokio::test]
#[ignore]
async fn live_search_inbox() {
    let client = client(&account());
    let boxes = client.mailboxes().await.unwrap();
    assert!(boxes.iter().any(|b| b.name.eq_ignore_ascii_case("INBOX")));
    let page = client
        .search("INBOX", &SearchQuery::default(), 5, None)
        .await
        .unwrap();
    println!(
        "INBOX uidvalidity {} first page: {:?}",
        page.uidvalidity,
        page.messages
            .iter()
            .map(|m| (m.uid, &m.subject))
            .collect::<Vec<_>>()
    );
    if let Some(first) = page.messages.first() {
        let (_, raw) = client
            .fetch_raw("INBOX", first.uid, Some(page.uidvalidity))
            .await
            .unwrap();
        assert!(!raw.is_empty());
        let after = client.summaries("INBOX", &[first.uid]).await.unwrap().1;
        assert_eq!(
            after[0].flags.contains(&"\\Seen".to_string()),
            first.flags.contains(&"\\Seen".to_string()),
            "reading must not change \\Seen"
        );
    }
}

#[tokio::test]
#[ignore]
async fn live_non_ascii_search() {
    let client = client(&account());
    let q = SearchQuery {
        subject: Some("caf\u{e9}".into()),
        ..Default::default()
    };
    match client.search("INBOX", &q, 5, None).await {
        Ok(p) => println!("non-ASCII search ok: {} result(s)", p.messages.len()),
        Err(e) => panic!("iCloud rejected CHARSET UTF-8 search: {e}"),
    }
}

#[tokio::test]
#[ignore]
async fn live_draft_round_trip() {
    let a = account();
    let imap = client(&a);
    let server = MailServer::new(
        imap.clone(),
        None,
        Arc::new(ToolSettings {
            identity: Identity {
                email: a.email.clone(),
                name: Some("mcp-icloud-mail live test".into()),
            },
            max_attachment_bytes: 1024 * 1024,
        }),
    );
    let v = json(
        &server
            .create_draft(params(json!({
                "to": [a.email],
                "subject": "mcp-icloud-mail live test draft (safe to delete)",
                "body_text": "Created by the live test suite."
            })))
            .await
            .unwrap(),
    );
    let drafts = v["mailbox"].as_str().unwrap().to_string();
    let uid = v["uid"].as_u64().expect("draft uid") as u32;
    println!("draft {drafts}/{uid}");
    let (_, raw) = imap.fetch_raw(&drafts, uid, None).await.unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("live test draft"));

    imap.delete_messages(&drafts, &[uid], false, None)
        .await
        .unwrap();
    let trash = imap.role_mailbox(Role::Trash).await.unwrap();
    let found = imap
        .search_uids(
            &trash,
            &format!(
                "HEADER Message-ID \"<{}>\"",
                v["message_id"].as_str().unwrap()
            ),
        )
        .await
        .unwrap();
    println!("in {trash}: {found:?}");
    if imap.capabilities().await.unwrap().uidplus && !found.is_empty() {
        imap.delete_messages(&trash, &found, true, None)
            .await
            .unwrap();
    }
}
