use crate::cache::{LruMessageCache, MessageCache};
use crate::config::MailConfig;
use crate::imap::{ImapClient, ImapSettings};
use crate::mail::build::Identity;
use crate::smtp::{Sender, SmtpSender};
use crate::tools::{MailServer, ToolSettings};
use mcp_server_core::combined::DynHandler;
use mcp_server_core::service::{Service, ServiceContext, ServiceEnv, mcp_service};
use std::sync::Arc;

/// The mail service, mounted by the server at `/mail`.
pub struct MailService {
    imap: Arc<ImapClient>,
    sender: Option<Arc<dyn Sender>>,
    settings: Arc<ToolSettings>,
}

impl MailService {
    pub fn new(
        imap: Arc<ImapClient>,
        sender: Option<Arc<dyn Sender>>,
        settings: Arc<ToolSettings>,
    ) -> Self {
        Self {
            imap,
            sender,
            settings,
        }
    }

    fn server(&self) -> MailServer {
        MailServer::new(
            self.imap.clone(),
            self.sender.clone(),
            self.settings.clone(),
        )
    }
}

impl Service for MailService {
    fn name(&self) -> &'static str {
        "mail"
    }

    fn tool_names(&self) -> Vec<String> {
        self.server().tool_names()
    }

    /// async_imap's trace output contains the LOGIN command, password included.
    fn log_caps(&self) -> &'static [&'static str] {
        &["async_imap"]
    }

    fn router(self: Arc<Self>, ctx: &ServiceContext) -> axum::Router {
        mcp_service(ctx, move || Ok(self.server()))
    }

    fn handler(&self) -> DynHandler {
        DynHandler::new(self.server())
    }
}

/// Reads the mail configuration and builds the service. Errors go to `env.errors`.
pub fn from_env(env: &mut ServiceEnv) -> Option<Arc<dyn Service>> {
    let cfg = MailConfig::from_env(env)?;
    let cache: Arc<dyn MessageCache> = Arc::new(LruMessageCache::new(cfg.cache_max_bytes));
    let imap = Arc::new(ImapClient::new(
        ImapSettings::icloud(
            cfg.username.clone(),
            cfg.app_password.clone(),
            cfg.imap_idle_timeout,
        ),
        cache,
    ));
    let sender: Option<Arc<dyn Sender>> = if cfg.allow_send {
        match SmtpSender::icloud(&cfg.username, &cfg.app_password) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                env.errors.push(format!("SMTP setup failed: {e}"));
                return None;
            }
        }
    } else {
        None
    };
    let settings = Arc::new(ToolSettings {
        identity: Identity {
            email: cfg.email,
            name: cfg.from_name,
        },
        max_attachment_bytes: cfg.max_attachment_bytes,
        send_lock: Default::default(),
    });
    Some(Arc::new(MailService::new(imap, sender, settings)))
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
    fn builds_from_env_with_send_disabled_by_default() {
        let mut e = env(&[
            ("ICLOUD_EMAIL", "me@icloud.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]);
        let svc = from_env(&mut e).expect("service");
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        assert_eq!(svc.name(), "mail");
        assert_eq!(svc.log_caps(), &["async_imap"]);
        let tools = svc.tool_names();
        assert!(tools.contains(&"search_messages".to_string()), "{tools:?}");
        assert!(!tools.contains(&"send_draft".to_string()), "{tools:?}");
    }

    /// The SMTP pool needs a Tokio runtime when it is dropped.
    #[tokio::test]
    async fn send_draft_is_registered_when_sending_is_allowed() {
        let mut e = env(&[
            ("ICLOUD_EMAIL", "me@icloud.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
            ("ICLOUD_ALLOW_SEND", "true"),
        ]);
        let svc = from_env(&mut e).expect("service");
        assert!(svc.tool_names().contains(&"send_draft".to_string()));
    }

    #[test]
    fn missing_config_is_reported_and_no_service_is_built() {
        let mut e = env(&[]);
        assert!(from_env(&mut e).is_none());
        assert!(
            e.errors.iter().any(|m| m == "ICLOUD_EMAIL is required"),
            "{:?}",
            e.errors
        );
    }
}
