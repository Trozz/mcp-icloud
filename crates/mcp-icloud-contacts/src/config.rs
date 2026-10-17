use mcp_server_core::config::EnvReader;
use secrecy::SecretString;
use url::Url;

/// MCP `serverInfo.name` of the contacts service.
pub const SERVICE_NAME: &str = "mcp-icloud-contacts";
pub const DEFAULT_CARDDAV_URL: &str = "https://contacts.icloud.com";

/// Contacts settings only: the server binary owns the core configuration (URL, OAuth, TLS, logging).
pub struct ContactsConfig {
    pub email: String,
    pub app_password: SecretString,
    pub carddav_url: Url,
}

impl ContactsConfig {
    pub fn from_env<F: Fn(&str) -> Option<String>>(env: &mut EnvReader<F>) -> Option<Self> {
        let errors_before = env.errors.len();
        let email = env.required("ICLOUD_EMAIL");
        if let Some(e) = &email
            && !e.contains('@')
        {
            env.errors
                .push("ICLOUD_EMAIL must be an email address".into());
        }
        let app_password = env.secret_required("ICLOUD_APP_PASSWORD");
        let raw_url = env
            .optional("CARDDAV_URL")
            .unwrap_or_else(|| DEFAULT_CARDDAV_URL.to_string());
        let carddav_url = match parse_carddav_url(&raw_url) {
            Ok(u) => Some(u),
            Err(msg) => {
                env.errors.push(msg);
                None
            }
        };
        if env.errors.len() != errors_before {
            return None;
        }
        Some(ContactsConfig {
            email: email?,
            app_password: app_password?,
            carddav_url: carddav_url?,
        })
    }
}

fn parse_carddav_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("CARDDAV_URL is not a valid URL: {e}"))?;
    if url.scheme() == "https" && url.host_str().is_some() {
        return Ok(url);
    }
    if url.scheme() == "http" && is_loopback(&url) {
        return Ok(url);
    }
    Err("CARDDAV_URL must use https (http is only allowed for loopback addresses)".into())
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::collections::HashMap;

    fn load(extra: &[(&str, &str)]) -> (Option<ContactsConfig>, Vec<String>) {
        let mut map: HashMap<String, String> = [
            ("ICLOUD_EMAIL", "user@example.com"),
            ("ICLOUD_APP_PASSWORD", "abcd-efgh-ijkl-mnop"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        for (k, v) in extra {
            map.insert(k.to_string(), v.to_string());
        }
        let mut env = EnvReader::new(move |k: &str| map.get(k).cloned());
        let c = ContactsConfig::from_env(&mut env);
        (c, env.errors)
    }

    #[test]
    fn defaults() {
        let (c, errors) = load(&[]);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert_eq!(c.email, "user@example.com");
        assert_eq!(c.app_password.expose_secret(), "abcd-efgh-ijkl-mnop");
        assert_eq!(c.carddav_url.as_str(), "https://contacts.icloud.com/");
    }

    #[test]
    fn carddav_url_can_point_at_a_loopback_test_server() {
        let (c, errors) = load(&[("CARDDAV_URL", "http://127.0.0.1:9999")]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.unwrap().carddav_url.as_str(), "http://127.0.0.1:9999/");
    }

    #[test]
    fn carddav_url_must_be_https_unless_loopback() {
        for ok in [
            "https://contacts.icloud.com",
            "http://localhost:8080",
            "http://[::1]:8080",
        ] {
            assert!(parse_carddav_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://contacts.icloud.com",
            "ftp://x",
            "not a url",
            "http://10.0.0.1",
        ] {
            assert!(parse_carddav_url(bad).is_err(), "{bad}");
        }
        let (c, errors) = load(&[("CARDDAV_URL", "http://contacts.icloud.com")]);
        assert!(c.is_none());
        assert!(
            errors.iter().any(|e| e.contains("CARDDAV_URL")),
            "{errors:?}"
        );
    }

    #[test]
    fn contacts_errors_are_reported_together() {
        let (c, errors) = load(&[("ICLOUD_EMAIL", "not-an-email"), ("CARDDAV_URL", "ftp://x")]);
        assert!(c.is_none());
        let joined = errors.join("\n");
        assert!(
            joined.contains("ICLOUD_EMAIL must be an email address"),
            "{joined}"
        );
        assert!(joined.contains("CARDDAV_URL"), "{joined}");
        assert!(!joined.contains("abcd-efgh"), "secret leaked: {joined}");
    }

    #[test]
    fn missing_app_password_is_reported() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("ICLOUD_EMAIL".into(), "user@example.com".into());
        let mut env = EnvReader::new(move |k: &str| map.get(k).cloned());
        assert!(ContactsConfig::from_env(&mut env).is_none());
        assert!(
            env.errors
                .iter()
                .any(|e| e == "ICLOUD_APP_PASSWORD is required"),
            "{:?}",
            env.errors
        );
    }

    #[test]
    fn reads_no_server_variables() {
        let read = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = read.clone();
        let mut env = EnvReader::new(move |k: &str| {
            seen.lock().unwrap().push(k.to_string());
            match k {
                "ICLOUD_EMAIL" => Some("user@example.com".to_string()),
                "ICLOUD_APP_PASSWORD" => Some("abcd-efgh-ijkl-mnop".to_string()),
                _ => None,
            }
        });
        assert!(
            ContactsConfig::from_env(&mut env).is_some(),
            "{:?}",
            env.errors
        );
        let read = read.lock().unwrap();
        for k in read.iter() {
            assert!(
                k.starts_with("ICLOUD_") || k.starts_with("CARDDAV_"),
                "contacts config read a server variable: {k}"
            );
        }
        assert!(!read.iter().any(|k| k == "ICLOUD_SERVICES"), "{read:?}");
    }
}
