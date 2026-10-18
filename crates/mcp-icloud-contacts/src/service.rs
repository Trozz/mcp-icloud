use crate::config::ContactsConfig;
use crate::index::Store;
use crate::tools::ContactsServer;
use chrono_tz::Tz;
use mcp_icloud_dav::client::{DavClient, DavSettings, Protocol};
use mcp_server_core::combined::DynHandler;
use mcp_server_core::service::{Service, ServiceContext, ServiceEnv, mcp_service};
use std::sync::Arc;

/// The contacts service, mounted by the server at `/contacts`.
pub struct ContactsService {
    store: Arc<Store>,
    timezone: Tz,
}

impl ContactsService {
    pub fn new(store: Arc<Store>, timezone: Tz) -> Self {
        Self { store, timezone }
    }

    fn server(&self) -> ContactsServer {
        ContactsServer::new(self.store.clone()).with_timezone(self.timezone)
    }
}

impl Service for ContactsService {
    fn name(&self) -> &'static str {
        "contacts"
    }

    fn tool_names(&self) -> Vec<String> {
        self.server().tool_names()
    }

    fn router(self: Arc<Self>, ctx: &ServiceContext) -> axum::Router {
        // Load the address book early so a bad app password shows up in the startup logs, not on
        // the first call. Only when a runtime is running: the server always has one, unit tests
        // may not.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let warm = self.store.clone();
            rt.spawn(async move {
                if let Err(e) = warm.open().await {
                    tracing::warn!(kind = e.kind(), error = %e, "contacts discovery failed; will retry on the first tool call");
                }
            });
        }
        mcp_service(ctx, move || Ok(self.server()))
    }

    fn handler(&self) -> DynHandler {
        DynHandler::new(self.server())
    }
}

/// Reads the contacts configuration and builds the service. Errors go to `env.errors`.
pub fn from_env(env: &mut ServiceEnv) -> Option<Arc<dyn Service>> {
    let cfg = ContactsConfig::from_env(env)?;
    let settings = DavSettings::new(
        Protocol::CardDav,
        cfg.carddav_url,
        cfg.email,
        cfg.app_password,
        format!("mcp-icloud-contacts/{}", crate::VERSION),
    );
    let client = match DavClient::new(settings) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            env.errors.push(format!("CardDAV client setup failed: {e}"));
            return None;
        }
    };
    Some(Arc::new(ContactsService::new(
        Arc::new(Store::new(client)),
        cfg.timezone,
    )))
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

    #[test]
    fn builds_from_env_with_eleven_tools() {
        let mut e = env(&[
            ("ICLOUD_EMAIL", "user@example.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]);
        let svc = from_env(&mut e).expect("service");
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        assert_eq!(svc.name(), "contacts");
        assert!(svc.log_caps().is_empty());
        assert_eq!(
            svc.tool_names(),
            [
                "create_contact",
                "create_group",
                "delete_contact",
                "delete_group",
                "get_contact",
                "get_my_contact",
                "list_groups",
                "search_contacts",
                "upcoming_dates",
                "update_contact",
                "update_group",
            ]
        );
    }

    #[test]
    fn missing_config_is_reported_and_no_service_is_built() {
        let mut e = env(&[("CARDDAV_URL", "ftp://x")]);
        assert!(from_env(&mut e).is_none());
        let joined = e.errors.join("\n");
        assert!(joined.contains("ICLOUD_EMAIL is required"), "{joined}");
        assert!(joined.contains("CARDDAV_URL"), "{joined}");
    }
}
