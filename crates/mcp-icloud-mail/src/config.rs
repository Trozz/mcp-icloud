use mcp_server_core::config::{CoreConfig, EnvReader};
use secrecy::SecretString;
use std::time::Duration;

pub const SERVICE_NAME: &str = "mcp-icloud-mail";
pub const CRATE_TARGET: &str = "mcp_icloud_mail";

pub struct MailConfig {
    pub core: CoreConfig,
    pub email: String,
    pub username: String,
    pub app_password: SecretString,
    pub from_name: Option<String>,
    pub allow_send: bool,
    pub max_attachment_bytes: usize,
    pub cache_max_bytes: usize,
    pub imap_idle_timeout: Duration,
}

impl MailConfig {
    pub fn from_env<F: Fn(&str) -> Option<String>>(env: &mut EnvReader<F>) -> Option<Self> {
        let core = CoreConfig::from_env(env, SERVICE_NAME, CRATE_TARGET);
        let errors_before = env.errors.len();
        let email = env.required("ICLOUD_EMAIL");
        if let Some(e) = &email
            && !e.contains('@')
        {
            env.errors
                .push("ICLOUD_EMAIL must be an email address".into());
        }
        let username = env.optional("ICLOUD_USERNAME");
        let app_password = env.secret_required("ICLOUD_APP_PASSWORD");
        let from_name = env.optional("ICLOUD_FROM_NAME");
        let allow_send = env.bool("ICLOUD_ALLOW_SEND", false);
        let max_attachment_bytes = env.parse("MAX_ATTACHMENT_BYTES", 10 * 1024 * 1024usize);
        let cache_max_bytes = env.parse("CACHE_MAX_BYTES", 64 * 1024 * 1024usize);
        let idle_secs = env.parse("IMAP_IDLE_TIMEOUT", 600u64);
        if env.errors.len() != errors_before {
            return None;
        }
        let core = core?;
        let email = email?;
        Some(MailConfig {
            core,
            username: username.unwrap_or_else(|| email.clone()),
            email,
            app_password: app_password?,
            from_name,
            allow_send,
            max_attachment_bytes,
            cache_max_bytes,
            imap_idle_timeout: Duration::from_secs(idle_secs),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::collections::HashMap;

    fn load(extra: &[(&str, &str)]) -> (Option<MailConfig>, Vec<String>) {
        let mut map: HashMap<String, String> = [
            ("PUBLIC_URL", "https://mail.example.com"),
            ("OAUTH_SIGNING_KEY", "0123456789abcdef0123456789abcdef"),
            ("OAUTH_ADMIN_PASSWORD", "a long password"),
            ("ICLOUD_EMAIL", "me@icloud.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        for (k, v) in extra {
            map.insert(k.to_string(), v.to_string());
        }
        let mut env = EnvReader::new(move |k: &str| map.get(k).cloned());
        let c = MailConfig::from_env(&mut env);
        (c, env.errors)
    }

    #[test]
    fn defaults() {
        let (c, errors) = load(&[]);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert_eq!(c.username, "me@icloud.com");
        assert_eq!(c.app_password.expose_secret(), "abcd-efgh-ijkl-mnop");
        assert!(!c.allow_send);
        assert_eq!(c.max_attachment_bytes, 10_485_760);
        assert_eq!(c.cache_max_bytes, 67_108_864);
        assert_eq!(c.imap_idle_timeout, Duration::from_secs(600));
        assert_eq!(c.core.observability.crate_target, "mcp_icloud_mail");
    }

    #[test]
    fn overrides() {
        let (c, errors) = load(&[
            ("ICLOUD_USERNAME", "login@me.com"),
            ("ICLOUD_ALLOW_SEND", "true"),
            ("ICLOUD_FROM_NAME", "Me"),
            ("CACHE_MAX_BYTES", "0"),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert_eq!(c.username, "login@me.com");
        assert!(c.allow_send);
        assert_eq!(c.from_name.as_deref(), Some("Me"));
        assert_eq!(c.cache_max_bytes, 0);
    }

    #[test]
    fn mail_and_core_errors_are_reported_together() {
        let (c, errors) = load(&[
            ("ICLOUD_EMAIL", "not-an-email"),
            ("PUBLIC_URL", "ftp://x"),
            ("MAX_ATTACHMENT_BYTES", "lots"),
        ]);
        assert!(c.is_none());
        let joined = errors.join("\n");
        assert!(
            joined.contains("ICLOUD_EMAIL must be an email address"),
            "{joined}"
        );
        assert!(joined.contains("PUBLIC_URL"), "{joined}");
        assert!(
            joined.contains("MAX_ATTACHMENT_BYTES has an invalid value"),
            "{joined}"
        );
        assert!(!joined.contains("abcd-efgh"), "secret leaked: {joined}");
    }

    #[test]
    fn missing_app_password_is_reported() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("ICLOUD_EMAIL".into(), "me@icloud.com".into());
        let mut env = EnvReader::new(move |k: &str| map.get(k).cloned());
        assert!(MailConfig::from_env(&mut env).is_none());
        assert!(
            env.errors
                .iter()
                .any(|e| e == "ICLOUD_APP_PASSWORD is required"),
            "{:?}",
            env.errors
        );
    }
}
