//! Live tests against a real iCloud account. Ignored by default; they need
//! ICLOUD_EMAIL and ICLOUD_APP_PASSWORD, for example through 1Password:
//!   op run --env-file="${APP_ENV:-$HOME/docker/mcp-icloud/app.env}" -- \
//!     cargo test -p mcp-icloud-contacts --test live_icloud -- --ignored --nocapture --test-threads=1
//! The round trips create contacts "ZZ MCP Probe" and a group "ZZ MCP Probe Group" with made-up
//! data (the merge test two such contacts, merged into one; the round trip also exports the probe
//! contact and group) and always delete them again, also when an assertion fails. Output only
//! prints counts, never contact content.
mod support;
use futures::FutureExt;
use mcp_icloud_contacts::config::DEFAULT_CARDDAV_URL;
use mcp_icloud_contacts::index::Store;
use mcp_icloud_contacts::tools::ContactsServer;
use mcp_icloud_dav::client::{DavClient, DavSettings, Protocol};
use secrecy::SecretString;
use serde_json::{Value, json};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use support::tools::{error_text, is_error, json, params, text, text_resource};
use url::Url;

const NAME: &str = "ZZ MCP Probe";
const GROUP_NAME: &str = "ZZ MCP Probe Group";

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// A server with its own empty index, so every call to it is a fresh read from iCloud.
fn server() -> ContactsServer {
    let email = env("ICLOUD_EMAIL").expect("set ICLOUD_EMAIL to run the live tests");
    let password =
        env("ICLOUD_APP_PASSWORD").expect("set ICLOUD_APP_PASSWORD to run the live tests");
    let client = DavClient::new(DavSettings::new(
        Protocol::CardDav,
        Url::parse(DEFAULT_CARDDAV_URL).unwrap(),
        email,
        SecretString::from(password),
        "mcp-icloud-contacts/live-test".into(),
    ))
    .unwrap();
    ContactsServer::new(Arc::new(Store::new(Arc::new(client))))
}

/// Deletes every contact named `NAME` and every group named `GROUP_NAME`.
async fn cleanup(s: &ContactsServer) {
    if let Ok(r) = s
        .search_contacts(params(json!({"query": NAME, "limit": 100})))
        .await
        && let Ok(v) = serde_json::from_str::<Value>(&text(&r))
    {
        for c in v["contacts"].as_array().into_iter().flatten() {
            if c["display_name"] == NAME {
                let _ = s.delete_contact(params(json!({"id": c["id"]}))).await;
            }
        }
    }
    if let Ok(r) = s.list_groups().await
        && let Ok(v) = serde_json::from_str::<Value>(&text(&r))
    {
        for g in v.as_array().into_iter().flatten() {
            if g["name"] == GROUP_NAME {
                let _ = s.delete_group(params(json!({"id": g["id"]}))).await;
            }
        }
    }
}

fn group_entry(groups: &Value, id: &str) -> Option<Value> {
    groups.as_array()?.iter().find(|g| g["id"] == id).cloned()
}

#[tokio::test]
#[ignore]
async fn discovery_search_and_my_card() {
    let s = server();
    let v = json(
        &s.search_contacts(params(json!({"limit": 5})))
            .await
            .unwrap(),
    );
    println!("contacts: {}", v["total"]);
    assert!(v["total"].as_u64().unwrap() > 0);
    let groups = json(&s.list_groups().await.unwrap());
    println!("groups: {}", groups.as_array().unwrap().len());
    let me = s.get_my_contact().await.unwrap();
    if is_error(&me) {
        println!("my card: none set");
        return;
    }
    let me = json(&me);
    assert_eq!(me["is_me"], true);
    println!("my card: present, {} fields", me.as_object().unwrap().len());
}

#[tokio::test]
#[ignore]
async fn round_trip_contact_and_group() {
    let s = server();
    cleanup(&s).await;
    let result = AssertUnwindSafe(round_trip(&s)).catch_unwind().await;
    cleanup(&s).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn round_trip(s: &ContactsServer) {
    let created = json(
        &s.create_contact(params(json!({
            "given_name": "ZZ MCP",
            "family_name": "Probe",
            "emails": [{"value": "probe@example.com", "label": "home"}],
            "phones": [{"value": "+44 7700 900123", "label": "mobile"}],
            "note": "Created by the mcp-icloud live test, safe to delete"
        })))
        .await
        .unwrap(),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let got = json(
        &server()
            .get_contact(params(json!({"id": id})))
            .await
            .unwrap(),
    );
    assert_eq!(got["display_name"], NAME);
    assert_eq!(got["phones"][0]["label"], "mobile");
    println!("created: ok");

    json(
        &s.update_contact(params(json!({
            "id": id,
            "note": "Edited by the mcp-icloud live test, safe to delete"
        })))
        .await
        .unwrap(),
    );
    let got = json(
        &server()
            .get_contact(params(json!({"id": id})))
            .await
            .unwrap(),
    );
    assert_eq!(
        got["note"],
        "Edited by the mcp-icloud live test, safe to delete"
    );
    assert_eq!(got["emails"][0]["value"], "probe@example.com");
    println!("updated: ok");

    let group = json(
        &s.create_group(params(json!({"name": GROUP_NAME, "member_ids": [id]})))
            .await
            .unwrap(),
    );
    let gid = group["id"].as_str().unwrap().to_string();
    let groups = json(&server().list_groups().await.unwrap());
    assert_eq!(group_entry(&groups, &gid).unwrap()["member_count"], 1);
    println!("group: ok");
    let exported = server()
        .export_vcard(params(json!({"id": id})))
        .await
        .unwrap();
    let meta = json(&exported);
    let (_, mime_type, vcf) = text_resource(&exported);
    assert_eq!(meta["kind"], "contact");
    assert_eq!(meta["cards"], 1);
    assert_eq!(meta["bytes"], vcf.len());
    assert_eq!(mime_type.as_deref(), Some("text/vcard"));
    assert!(vcf.starts_with("BEGIN:VCARD\r\n"), "export is not a vCard");
    assert!(vcf.ends_with("END:VCARD\r\n"), "export is not a vCard");
    println!("contact export: {} bytes", vcf.len());
    let exported = server()
        .export_vcard(params(json!({"id": gid})))
        .await
        .unwrap();
    let group_meta = json(&exported);
    let (_, _, group_vcf) = text_resource(&exported);
    assert_eq!(group_meta["kind"], "group");
    assert_eq!(group_meta["cards"], 1);
    assert_eq!(group_meta["missing_members"], json!([]));
    assert!(
        group_vcf == vcf,
        "the group export is the probe contact's card"
    );
    println!(
        "group export: {} cards, {} bytes",
        group_meta["cards"],
        group_vcf.len()
    );

    let deleted = json(&s.delete_contact(params(json!({"id": id}))).await.unwrap());
    assert_eq!(deleted["groups_updated"], json!([gid]));
    let fresh = server();
    let e = error_text(&fresh.get_contact(params(json!({"id": id}))).await.unwrap());
    assert!(e.starts_with("not_found:"), "{e}");
    let groups = json(&fresh.list_groups().await.unwrap());
    assert_eq!(group_entry(&groups, &gid).unwrap()["member_count"], 0);
    println!("contact deleted, group cleaned: ok");

    json(&s.delete_group(params(json!({"id": gid}))).await.unwrap());
    let groups = json(&server().list_groups().await.unwrap());
    assert!(group_entry(&groups, &gid).is_none());
    println!("group deleted: ok");
}

#[tokio::test]
#[ignore]
async fn merge_two_probe_contacts() {
    let s = server();
    cleanup(&s).await;
    let result = AssertUnwindSafe(merge_round_trip(&s)).catch_unwind().await;
    cleanup(&s).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn merge_round_trip(s: &ContactsServer) {
    let create = |extra: Value| {
        let mut args = json!({
            "given_name": "ZZ MCP",
            "family_name": "Probe",
            "emails": [{"value": "probe@example.com", "label": "home"}],
        });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };
    let keep = json(&s.create_contact(params(create(json!({})))).await.unwrap());
    let other = json(
        &s.create_contact(params(create(json!({
            "phones": [{"value": "+44 7700 900123", "label": "mobile"}],
            "note": "Created by the mcp-icloud live test, safe to delete"
        }))))
        .await
        .unwrap(),
    );
    let (keep_id, other_id) = (keep["id"].as_str().unwrap(), other["id"].as_str().unwrap());
    let found = json(&server().find_duplicate_contacts().await.unwrap());
    let group = found["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| {
            g["contacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == keep_id)
        })
        .expect("the probe contacts are found as duplicates");
    assert_eq!(group["contacts"].as_array().unwrap().len(), 2);
    println!("duplicates found: ok");

    let merged = json(
        &s.merge_contacts(params(json!({"keep_id": keep_id, "merge_ids": [other_id]})))
            .await
            .unwrap(),
    );
    assert_eq!(merged["not_deleted"], json!([]));
    let fresh = server();
    let got = json(
        &fresh
            .get_contact(params(json!({"id": keep_id})))
            .await
            .unwrap(),
    );
    assert_eq!(got["phones"][0]["value"], "+44 7700 900123");
    assert_eq!(
        got["note"],
        "Created by the mcp-icloud live test, safe to delete"
    );
    let e = error_text(
        &fresh
            .get_contact(params(json!({"id": other_id})))
            .await
            .unwrap(),
    );
    assert!(e.starts_with("not_found:"), "{e}");
    println!("merged: ok");
}
