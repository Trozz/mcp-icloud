use mcp_server_core::combined::{Combined, DynHandler};
use mcp_server_core::config::CoreConfig;
use mcp_server_core::oauth::{AuthState, OAuthSettings};
use mcp_server_core::policy::{AuditLog, Guarded, ToolPolicy};
use mcp_server_core::server::build_app;
use mcp_server_core::service::{
    Service, ServiceContext, ServiceEnv, ServiceFactory, icons, mcp_service,
};
use rmcp::model::Implementation;
use std::path::PathBuf;
use std::sync::Arc;

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub const SERVICE_NAME: &str = "mcp-icloud";
/// Tracing target prefix: also matches `mcp_icloud_mail` and every other `mcp_icloud_*` crate.
pub const CRATE_TARGET: &str = "mcp_icloud";
/// Path, OAuth resource suffix and scope of the combined endpoint; never a service name.
pub const COMBINED: &str = "mcp";

/// Every service this binary can run, in mount order. Adding a service: a library crate
/// implementing `mcp_server_core::service::Service`, a dependency in Cargo.toml and one line here.
pub const SERVICES: &[(&str, ServiceFactory)] = &[
    ("mail", mcp_icloud_mail::from_env),
    ("calendar", mcp_icloud_calendar::from_env),
    ("contacts", mcp_icloud_contacts::from_env),
];

/// Resolves `ICLOUD_SERVICES` (comma list, unset means every registered service).
pub fn select<'a>(
    raw: Option<&str>,
    registry: &'a [(&'static str, ServiceFactory)],
) -> Result<Vec<&'a (&'static str, ServiceFactory)>, String> {
    let Some(raw) = raw else {
        return Ok(registry.iter().collect());
    };
    let known = || {
        registry
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut selected: Vec<&(&'static str, ServiceFactory)> = Vec::new();
    for name in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some(entry) = registry.iter().find(|(n, _)| *n == name) else {
            return Err(format!(
                "ICLOUD_SERVICES contains an unknown service: {name} (known: {})",
                known()
            ));
        };
        if !selected.iter().any(|(n, _)| *n == name) {
            selected.push(entry);
        }
    }
    if selected.is_empty() {
        return Err(format!(
            "ICLOUD_SERVICES must name at least one service (known: {})",
            known()
        ));
    }
    Ok(selected)
}

/// Resolves `ICLOUD_READ_ONLY` (comma list or `all`, unset or empty means none) against the
/// registered names. Names that are registered but not selected are accepted and have no effect.
pub fn read_only(
    raw: Option<&str>,
    registry: &[(&'static str, ServiceFactory)],
) -> Result<Vec<&'static str>, String> {
    let names = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let mut out: Vec<&'static str> = Vec::new();
    for name in names {
        if name == "all" {
            return Ok(registry.iter().map(|(n, _)| *n).collect());
        }
        let Some((known, _)) = registry.iter().find(|(n, _)| *n == name) else {
            let known: Vec<&str> = registry.iter().map(|(n, _)| *n).collect();
            return Err(format!(
                "ICLOUD_READ_ONLY contains an unknown service: {name} (known: all, {})",
                known.join(", ")
            ));
        };
        if !out.contains(known) {
            out.push(known);
        }
    }
    Ok(out)
}

/// The read-only services and the audit log, applied to every service's tools.
pub struct Safety {
    pub read_only: Vec<&'static str>,
    pub audit: Arc<AuditLog>,
}

impl Safety {
    pub fn is_read_only(&self, service: &str) -> bool {
        self.read_only.contains(&service)
    }

    /// The policy for `service` on its own endpoint `/<service>`.
    pub fn policy(&self, service: &str) -> ToolPolicy {
        ToolPolicy {
            service: service.to_string(),
            endpoint: format!("/{service}"),
            read_only: self.is_read_only(service),
            audit: self.audit.clone(),
        }
    }
}

pub struct Loaded {
    pub core: CoreConfig,
    pub services: Vec<Arc<dyn Service>>,
    pub safety: Safety,
    /// Serve every service's tools together at `/mcp` (`ICLOUD_COMBINED`, default true).
    pub combined: bool,
}

/// Reads the core configuration and builds every selected service. All errors are collected in
/// `env.errors`; `None` means the server cannot start. Unselected services never read their config.
pub fn load(env: &mut ServiceEnv, registry: &[(&'static str, ServiceFactory)]) -> Option<Loaded> {
    let core = CoreConfig::from_env(env, SERVICE_NAME, CRATE_TARGET);
    let raw = env.optional("ICLOUD_SERVICES");
    let selected = match select(raw.as_deref(), registry) {
        Ok(s) => s,
        Err(e) => {
            env.errors.push(e);
            Vec::new()
        }
    };
    let read_only = match read_only(env.optional("ICLOUD_READ_ONLY").as_deref(), registry) {
        Ok(r) => r,
        Err(e) => {
            env.errors.push(e);
            Vec::new()
        }
    };
    let audit_path = env.optional("AUDIT_LOG_PATH").map(PathBuf::from);
    let combined = env.bool("ICLOUD_COMBINED", true);
    let mut services = Vec::new();
    for (_, factory) in selected {
        if let Some(s) = factory(env) {
            services.push(s);
        }
    }
    // Services share some variables (ICLOUD_EMAIL, ICLOUD_APP_PASSWORD): report each error once.
    let mut seen = std::collections::HashSet::new();
    env.errors.retain(|e| seen.insert(e.clone()));
    if !env.errors.is_empty() {
        return None;
    }
    Some(Loaded {
        core: core?,
        services,
        safety: Safety {
            read_only,
            audit: Arc::new(AuditLog::new(audit_path)),
        },
        combined,
    })
}

/// `serverInfo` of the combined endpoint: this binary's name, version and icon.
fn combined_info() -> Implementation {
    Implementation::new(SERVICE_NAME, VERSION).with_icons(icons(
        include_bytes!("../assets/favicon.png"),
        include_str!("../assets/favicon.svg"),
    ))
}

/// The combined endpoint: every service's tools as `<service>_<tool>`, each service under its own
/// read-only rule and audit log, recorded with endpoint `/mcp`.
fn combined_router(
    services: &[Arc<dyn Service>],
    ctx: &ServiceContext,
    safety: &Safety,
) -> axum::Router {
    let parts: Vec<(Arc<dyn Service>, ToolPolicy)> = services
        .iter()
        .map(|s| {
            let policy = ToolPolicy {
                endpoint: format!("/{COMBINED}"),
                ..safety.policy(s.name())
            };
            (s.clone(), policy)
        })
        .collect();
    mcp_service(ctx, move || {
        let handlers = parts
            .iter()
            .map(|(s, policy)| {
                let guarded = Guarded::with_policy(s.handler(), policy.clone());
                (s.name(), DynHandler::new(guarded))
            })
            .collect();
        Ok(Combined::new(combined_info(), handlers))
    })
}

/// The full HTTP application: health, favicon, OAuth routes and each service at `/<name>`, every
/// service its own OAuth protected resource with its read-only rule and audit log; with
/// `combined`, also every service's tools together at `/mcp`, its own protected resource.
pub fn app(
    oauth: OAuthSettings,
    services: &[Arc<dyn Service>],
    ctx: &ServiceContext,
    safety: &Safety,
    combined: bool,
) -> axum::Router {
    assert!(
        services.iter().all(|s| s.name() != COMBINED),
        "{COMBINED} is reserved for the combined endpoint"
    );
    let combined = combined && !services.is_empty();
    let mut names: Vec<&str> = services.iter().map(|s| s.name()).collect();
    if combined {
        names.push(COMBINED);
    }
    let auth = AuthState::new(oauth, &names);
    let mut routers: Vec<(&'static str, axum::Router)> = services
        .iter()
        .map(|s| {
            let policy = axum::Extension(safety.policy(s.name()));
            (s.name(), s.clone().router(ctx).layer(policy))
        })
        .collect();
    if combined {
        routers.push((COMBINED, combined_router(services, ctx, safety)));
    }
    build_app(auth, routers).route(
        "/favicon.ico",
        axum::routing::get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "image/png")],
                include_bytes!("../assets/favicon.png").as_slice(),
            )
        }),
    )
}

/// Drops any user directive for a capped target and caps it at info: some dependencies log
/// secrets at trace (async_imap logs the LOGIN password). Directives with a `[` are dropped too:
/// span-scoped ones could re-enable a capped target.
pub fn log_filter(level: &str, caps: &[&str]) -> String {
    let mut parts: Vec<String> = level
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.contains('[') && !caps.iter().any(|c| d.starts_with(c)))
        .map(String::from)
        .collect();
    parts.extend(caps.iter().map(|c| format!("{c}=info")));
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn needy(env: &mut ServiceEnv) -> Option<Arc<dyn Service>> {
        env.required("NEEDY_TOKEN");
        None
    }

    const TWO: &[(&str, ServiceFactory)] = &[("mail", mcp_icloud_mail::from_env), ("needy", needy)];

    fn names(v: &[&(&'static str, ServiceFactory)]) -> Vec<&'static str> {
        v.iter().map(|(n, _)| *n).collect()
    }

    #[test]
    fn unset_selects_every_registered_service() {
        assert_eq!(names(&select(None, TWO).unwrap()), vec!["mail", "needy"]);
    }

    #[test]
    fn explicit_selection_is_trimmed_and_deduplicated() {
        assert_eq!(
            names(&select(Some(" needy , mail,needy"), TWO).unwrap()),
            vec!["needy", "mail"]
        );
    }

    #[test]
    fn unknown_or_empty_selection_is_an_error() {
        let e = select(Some("mail,contacts"), TWO).unwrap_err();
        assert_eq!(
            e,
            "ICLOUD_SERVICES contains an unknown service: contacts (known: mail, needy)"
        );
        let e = select(Some(" , "), TWO).unwrap_err();
        assert!(
            e.starts_with("ICLOUD_SERVICES must name at least one service"),
            "{e}"
        );
    }

    #[test]
    fn unselected_services_do_not_read_config() {
        let mut e = with_core(&[("ICLOUD_SERVICES", "mail")]);
        let loaded = load(&mut e, TWO).expect("loads");
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        assert_eq!(loaded.services.len(), 1);
        assert_eq!(loaded.services[0].name(), "mail");

        let mut e = with_core(&[]);
        assert!(load(&mut e, TWO).is_none());
        assert_eq!(e.errors, vec!["NEEDY_TOKEN is required".to_string()]);
    }

    #[test]
    fn core_and_service_errors_are_reported_together() {
        let mut e = env(&[
            ("PUBLIC_URL", "ftp://x"),
            ("ICLOUD_EMAIL", "not-an-email"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]);
        assert!(load(&mut e, SERVICES).is_none());
        let joined = e.errors.join("\n");
        assert!(joined.contains("PUBLIC_URL"), "{joined}");
        assert!(joined.contains("OAUTH_SIGNING_KEY is required"), "{joined}");
        assert!(
            joined.contains("ICLOUD_EMAIL must be an email address"),
            "{joined}"
        );
        assert!(!joined.contains("abcd-efgh"), "secret leaked: {joined}");
    }

    #[test]
    fn selection_errors_are_reported_with_the_rest() {
        let mut e = with_core(&[("ICLOUD_SERVICES", "reminders")]);
        assert!(load(&mut e, SERVICES).is_none());
        assert_eq!(
            e.errors,
            vec![
                "ICLOUD_SERVICES contains an unknown service: reminders (known: mail, calendar, contacts)"
                    .to_string()
            ]
        );
    }

    #[test]
    fn shared_variable_errors_are_reported_once() {
        let mut e = env(&[
            ("PUBLIC_URL", "https://icloud.example.com"),
            ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
            ("OAUTH_ADMIN_PASSWORD", "a long password"),
        ]);
        assert!(load(&mut e, SERVICES).is_none());
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
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{name} must be a lowercase path segment"
            );
        }
    }

    #[test]
    fn no_service_takes_the_combined_endpoints_name() {
        assert!(SERVICES.iter().all(|(name, _)| *name != COMBINED));
        let e = select(Some(COMBINED), SERVICES).unwrap_err();
        assert!(
            e.starts_with("ICLOUD_SERVICES contains an unknown service: mcp"),
            "{e}"
        );
    }

    #[test]
    fn the_combined_endpoint_is_on_unless_turned_off() {
        let mut e = with_core(&[]);
        assert!(load(&mut e, SERVICES).expect("loads").combined);
        let mut e = with_core(&[("ICLOUD_COMBINED", "false")]);
        assert!(!load(&mut e, SERVICES).expect("loads").combined);
        let mut e = with_core(&[("ICLOUD_COMBINED", "maybe")]);
        assert!(load(&mut e, SERVICES).is_none());
        assert_eq!(e.errors.len(), 1, "{:?}", e.errors);
        assert!(e.errors[0].contains("ICLOUD_COMBINED"), "{:?}", e.errors);
    }

    #[test]
    fn read_only_accepts_names_all_and_nothing() {
        assert_eq!(read_only(None, SERVICES).unwrap(), Vec::<&str>::new());
        assert_eq!(
            read_only(Some(" , "), SERVICES).unwrap(),
            Vec::<&str>::new()
        );
        assert_eq!(
            read_only(Some(" contacts,mail,contacts"), SERVICES).unwrap(),
            vec!["contacts", "mail"]
        );
        assert_eq!(
            read_only(Some("mail,all"), SERVICES).unwrap(),
            vec!["mail", "calendar", "contacts"]
        );
    }

    #[test]
    fn unknown_read_only_names_stop_startup() {
        let mut e = with_core(&[("ICLOUD_READ_ONLY", "contacts,reminders")]);
        assert!(load(&mut e, SERVICES).is_none());
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
        let loaded = load(&mut e, SERVICES).expect("loads");
        assert!(loaded.safety.is_read_only("contacts"));
        assert!(!loaded.safety.is_read_only("mail"));
        assert!(!loaded.safety.policy("mail").read_only);
        assert_eq!(loaded.safety.policy("mail").endpoint, "/mail");
    }

    #[test]
    fn the_audit_target_stays_on_at_any_level() {
        use mcp_server_core::policy::AUDIT_TARGET;
        use tracing::Level;
        use tracing_subscriber::layer::SubscriberExt;
        for level in [
            "warn",
            "error",
            "off",
            "info,audit=off",
            "trace,audit=error",
        ] {
            let directives = mcp_server_core::observability::build_filter(
                &log_filter(level, &[AUDIT_TARGET]),
                false,
                CRATE_TARGET,
            );
            let subscriber =
                tracing_subscriber::registry().with(tracing_subscriber::EnvFilter::new(directives));
            tracing::subscriber::with_default(subscriber, || {
                assert!(tracing::enabled!(target: "audit", Level::INFO), "{level}");
            });
        }
    }

    #[test]
    fn async_imap_is_always_capped_at_info() {
        let caps = ["async_imap"];
        assert_eq!(log_filter("info", &caps), "info,async_imap=info");
        assert_eq!(log_filter("trace", &caps), "trace,async_imap=info");
        assert_eq!(log_filter("", &caps), "async_imap=info");
        assert_eq!(
            log_filter("debug,async_imap=trace,async_imap::client=trace", &caps),
            "debug,async_imap=info"
        );
        assert_eq!(
            log_filter("info,[{x}]=trace,[x]=trace,debug", &caps),
            "info,debug,async_imap=info"
        );
    }

    #[test]
    fn log_filter_caps_every_enabled_service() {
        assert_eq!(
            log_filter("trace,a=trace,b::x=debug", &["a", "b"]),
            "trace,a=info,b=info"
        );
        assert_eq!(log_filter("warn", &[]), "warn");
    }

    #[test]
    fn debug_directive_covers_service_crates() {
        use tracing::Level;
        use tracing_subscriber::layer::SubscriberExt;
        let level = log_filter("trace", &["async_imap"]);
        let directives = mcp_server_core::observability::build_filter(&level, true, CRATE_TARGET);
        let subscriber =
            tracing_subscriber::registry().with(tracing_subscriber::EnvFilter::new(directives));
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: "mcp_icloud_mail::imap", Level::DEBUG));
            assert!(tracing::enabled!(target: "mcp_icloud", Level::DEBUG));
            assert!(!tracing::enabled!(target: "async_imap::client", Level::DEBUG));
            assert!(tracing::enabled!(target: "async_imap::client", Level::INFO));
        });
        let directives = mcp_server_core::observability::build_filter("info", true, CRATE_TARGET);
        let subscriber =
            tracing_subscriber::registry().with(tracing_subscriber::EnvFilter::new(directives));
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: "mcp_icloud_mail::tools", Level::DEBUG));
            assert!(!tracing::enabled!(target: "hyper", Level::DEBUG));
        });
    }
}
