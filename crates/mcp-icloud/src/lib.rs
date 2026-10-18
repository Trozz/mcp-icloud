//! The mcp-icloud server: every iCloud service on one [`mcp_server_core::app::Server`], signed in
//! to by its owner.

use mcp_server_core::app::{Server, SignIn};
pub use mcp_server_core::combined::COMBINED;
use mcp_server_core::service::ServiceFactory;

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub const SERVICE_NAME: &str = "mcp-icloud";
/// Tracing target prefix: also matches `mcp_icloud_mail` and every other `mcp_icloud_*` crate.
pub const CRATE_TARGET: &str = "mcp_icloud";

/// Every service this binary can run, in mount order. Adding a service: a library crate
/// implementing `mcp_server_core::service::Service`, a dependency in Cargo.toml and one line here.
pub const SERVICES: &[(&str, ServiceFactory)] = &[
    ("mail", mcp_icloud_mail::from_env),
    ("calendar", mcp_icloud_calendar::from_env),
    ("contacts", mcp_icloud_contacts::from_env),
];

/// The server: `ICLOUD_SERVICES`, `ICLOUD_COMBINED` and `ICLOUD_READ_ONLY` choose what runs, and
/// the owner signs in with `OAUTH_ADMIN_PASSWORD` (or `MCP_AUTH_TOKEN`).
pub fn server() -> Server {
    Server::new(SERVICE_NAME, VERSION)
        .env_prefix("ICLOUD")
        .crate_target(CRATE_TARGET)
        .services(SERVICES)
        .sign_in(SignIn::OwnerPassword)
        .icon(
            include_bytes!("../assets/favicon.png"),
            include_str!("../assets/favicon.svg"),
        )
        .docs_url("https://trozz.github.io/mcp-icloud/configuration.html")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_server_core::service::ServiceEnv;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> ServiceEnv {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ServiceEnv::new(Box::new(move |k: &str| map.get(k).cloned()))
    }

    const CORE: &[(&str, &str)] = &[
        ("PUBLIC_URL", "https://icloud.example.com"),
        ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
        ("OAUTH_ADMIN_PASSWORD", "a long password"),
        ("ICLOUD_EMAIL", "me@icloud.com"),
        ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
    ];

    fn with_core(extra: &[(&str, &str)]) -> ServiceEnv {
        let mut all = CORE.to_vec();
        all.extend_from_slice(extra);
        env(&all)
    }

    #[test]
    fn the_owner_signs_in_with_the_admin_password() {
        let mut e = with_core(&[]);
        let loaded = server().load(&mut e).expect("loads");
        assert!(!loaded.verifier.carried());
        assert_eq!(loaded.verifier.label(), "password");
        assert_eq!(
            loaded.services.iter().map(|s| s.name()).collect::<Vec<_>>(),
            ["mail", "calendar", "contacts"]
        );
    }

    #[test]
    fn core_and_service_errors_are_reported_together() {
        let mut e = env(&[
            ("PUBLIC_URL", "ftp://x"),
            ("ICLOUD_EMAIL", "not-an-email"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]);
        assert!(server().load(&mut e).is_none());
        let joined = e.errors.join("\n");
        assert!(joined.contains("PUBLIC_URL"), "{joined}");
        assert!(joined.contains("OAUTH_SIGNING_KEY is required"), "{joined}");
        assert!(
            joined.contains("OAUTH_ADMIN_PASSWORD is required"),
            "{joined}"
        );
        assert!(
            joined.contains("ICLOUD_EMAIL must be an email address"),
            "{joined}"
        );
        assert!(!joined.contains("abcd-efgh"), "secret leaked: {joined}");
    }

    #[test]
    fn short_admin_password_is_rejected_without_echoing_it() {
        let mut e = with_core(&[("OAUTH_ADMIN_PASSWORD", "elevenchars")]);
        assert!(server().load(&mut e).is_none());
        let joined = e.errors.join("\n");
        assert!(
            joined.contains("OAUTH_ADMIN_PASSWORD must be at least 12 characters"),
            "{joined}"
        );
        assert!(!joined.contains("elevenchars"), "secret leaked: {joined}");
        let mut e = with_core(&[("OAUTH_ADMIN_PASSWORD", "twelve chars")]);
        assert!(server().load(&mut e).is_some(), "{:?}", e.errors);
    }

    #[test]
    fn selection_errors_are_reported_with_the_rest() {
        let mut e = with_core(&[("ICLOUD_SERVICES", "reminders")]);
        assert!(server().load(&mut e).is_none());
        assert_eq!(
            e.errors,
            vec![
                "ICLOUD_SERVICES contains an unknown service: reminders (known: mail, calendar, contacts)"
                    .to_string()
            ]
        );
    }

    #[test]
    fn unselected_services_do_not_read_config() {
        let mut e = env(&[
            ("PUBLIC_URL", "https://icloud.example.com"),
            ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
            ("OAUTH_ADMIN_PASSWORD", "a long password"),
            ("ICLOUD_SERVICES", "calendar"),
            ("ICLOUD_EMAIL", "me@icloud.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
            ("ICLOUD_ALLOW_SEND", "maybe"),
        ]);
        let loaded = server().load(&mut e).expect("mail's settings are not read");
        assert_eq!(loaded.services.len(), 1);
    }

    #[test]
    fn shared_variable_errors_are_reported_once() {
        let mut e = env(&CORE[..3]);
        assert!(server().load(&mut e).is_none());
        assert_eq!(
            e.errors,
            vec![
                "ICLOUD_EMAIL is required".to_string(),
                "ICLOUD_APP_PASSWORD is required".to_string()
            ]
        );
    }

    #[test]
    fn registry_names_match_service_names() {
        for (name, factory) in SERVICES {
            let mut e = with_core(&[]);
            let svc = factory(&mut e).unwrap_or_else(|| panic!("{name}: {:?}", e.errors));
            assert_eq!(svc.name(), *name);
            assert_ne!(*name, COMBINED, "{COMBINED} is the combined endpoint");
        }
    }

    #[test]
    fn the_combined_endpoint_is_on_unless_turned_off() {
        let mut e = with_core(&[]);
        assert!(server().load(&mut e).expect("loads").combined);
        let mut e = with_core(&[("ICLOUD_COMBINED", "false")]);
        assert!(!server().load(&mut e).expect("loads").combined);
    }

    #[test]
    fn unknown_read_only_names_stop_startup() {
        let mut e = with_core(&[("ICLOUD_READ_ONLY", "contacts,reminders")]);
        assert!(server().load(&mut e).is_none());
        assert_eq!(
            e.errors,
            vec![
                "ICLOUD_READ_ONLY contains an unknown service: reminders (known: all, mail, calendar, contacts)"
                    .to_string()
            ]
        );
    }

    #[test]
    fn read_only_names_need_not_be_selected() {
        let mut e = with_core(&[
            ("ICLOUD_SERVICES", "mail"),
            ("ICLOUD_READ_ONLY", "contacts"),
        ]);
        let loaded = server().load(&mut e).expect("loads");
        assert!(loaded.safety.is_read_only("contacts"));
        assert!(!loaded.safety.is_read_only("mail"));
        assert!(!loaded.safety.policy("mail").read_only);
        assert_eq!(loaded.safety.policy("mail").endpoint, "/mail");
    }

    #[test]
    fn async_imap_is_always_capped_at_info_and_the_audit_target_kept_on() {
        let mut e = with_core(&[]);
        let loaded = server().load(&mut e).expect("loads");
        let caps = Server::log_caps(&loaded.services);
        assert!(caps.contains(&"async_imap"), "{caps:?}");
        assert!(
            caps.contains(&mcp_server_core::policy::AUDIT_TARGET),
            "{caps:?}"
        );
        let filter = mcp_server_core::observability::log_filter(
            "debug,async_imap=trace,async_imap::client=trace",
            &caps,
        );
        assert!(filter.starts_with("debug,async_imap=info,"), "{filter}");
    }

    #[test]
    fn debug_directive_covers_service_crates() {
        use tracing::Level;
        use tracing_subscriber::layer::SubscriberExt;
        let level = mcp_server_core::observability::log_filter("trace", &["async_imap"]);
        let directives = mcp_server_core::observability::build_filter(&level, true, CRATE_TARGET);
        let subscriber =
            tracing_subscriber::registry().with(tracing_subscriber::EnvFilter::new(directives));
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: "mcp_icloud_mail::imap", Level::DEBUG));
            assert!(tracing::enabled!(target: "mcp_icloud", Level::DEBUG));
            assert!(!tracing::enabled!(target: "async_imap::client", Level::DEBUG));
            assert!(tracing::enabled!(target: "async_imap::client", Level::INFO));
        });
    }
}
