use crate::caldav::caldav_settings;
use crate::caldav::discovery::Directory;
use crate::config::CalendarConfig;
use crate::tools::{CalendarServer, ToolSettings};
use crate::tz::ZoneState;
use mcp_icloud_dav::client::DavClient;
use mcp_server_core::combined::DynHandler;
use mcp_server_core::service::{Service, ServiceContext, ServiceEnv, mcp_service};
use std::sync::Arc;

/// The calendar service, mounted by the server at `/calendar`.
pub struct CalendarService {
    dir: Arc<Directory>,
    settings: Arc<ToolSettings>,
}

impl CalendarService {
    pub fn new(dir: Arc<Directory>, settings: Arc<ToolSettings>) -> Self {
        Self { dir, settings }
    }

    fn server(&self) -> CalendarServer {
        CalendarServer::new(self.dir.clone(), self.settings.clone())
    }
}

impl Service for CalendarService {
    fn name(&self) -> &'static str {
        "calendar"
    }

    fn tool_names(&self) -> Vec<String> {
        self.server().tool_names()
    }

    fn router(self: Arc<Self>, ctx: &ServiceContext) -> axum::Router {
        tracing::info!(
            invites_enabled = self.settings.allow_invites,
            timezone = self.settings.zones.default_zone().name(),
            "calendar settings"
        );
        // Discover early so a bad app password shows up in the startup logs, not on the first
        // call. Only when a runtime is running: the server always has one, unit tests may not.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let warm = self.dir.clone();
            rt.spawn(async move {
                if let Err(e) = warm.account().await {
                    tracing::warn!(kind = e.kind(), error = %e, "calendar discovery failed; will retry on the first tool call");
                }
            });
        }
        mcp_service(ctx, move || Ok(self.server()))
    }

    fn handler(&self) -> DynHandler {
        DynHandler::new(self.server())
    }
}

/// Reads the calendar configuration and builds the service. Errors go to `env.errors`.
pub fn from_env(env: &mut ServiceEnv) -> Option<Arc<dyn Service>> {
    let cfg = CalendarConfig::from_env(env)?;
    let caldav = caldav_settings(
        cfg.caldav_url.clone(),
        cfg.email.clone(),
        cfg.app_password.clone(),
    );
    let client = match DavClient::new(caldav) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            env.errors.push(format!("CalDAV client setup failed: {e}"));
            return None;
        }
    };
    let dir = Arc::new(Directory::new(client, cfg.email.clone()));
    let settings = Arc::new(ToolSettings {
        zones: ZoneState::new(cfg.timezone),
        allow_invites: cfg.allow_invites,
        email: cfg.email,
    });
    Some(Arc::new(CalendarService::new(dir, settings)))
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
    fn builds_from_env_with_invitations_disabled_by_default() {
        let mut e = env(&[
            ("ICLOUD_EMAIL", "user@example.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]);
        let svc = from_env(&mut e).expect("service");
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        assert_eq!(svc.name(), "calendar");
        assert!(svc.log_caps().is_empty());
        let tools = svc.tool_names();
        assert_eq!(tools.len(), 12, "{tools:?}");
        assert!(tools.contains(&"list_events".to_string()), "{tools:?}");
    }

    #[test]
    fn respond_to_invitation_is_registered_only_with_invitations_enabled() {
        let mut e = env(&[
            ("ICLOUD_EMAIL", "user@example.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
            ("ICLOUD_ALLOW_INVITES", "true"),
        ]);
        let tools = from_env(&mut e).expect("service").tool_names();
        assert_eq!(tools.len(), 13, "{tools:?}");
        assert!(tools.contains(&"respond_to_invitation".to_string()));
    }

    #[test]
    fn missing_config_is_reported_and_no_service_is_built() {
        let mut e = env(&[("CALENDAR_TIMEZONE", "Mars/Olympus")]);
        assert!(from_env(&mut e).is_none());
        let joined = e.errors.join("\n");
        assert!(joined.contains("ICLOUD_EMAIL is required"), "{joined}");
        assert!(joined.contains("CALENDAR_TIMEZONE"), "{joined}");
    }
}
