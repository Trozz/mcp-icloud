use chrono_tz::Tz;
use mcp_server_core::config::EnvReader;
use secrecy::SecretString;
use url::Url;

/// MCP `serverInfo.name` of the calendar service.
pub const SERVICE_NAME: &str = "mcp-icloud-calendar";
pub const DEFAULT_TIMEZONE: &str = "Europe/London";
pub const DEFAULT_CALDAV_URL: &str = "https://caldav.icloud.com";

/// Calendar settings only: the server binary owns the core configuration (URL, OAuth, TLS, logging).
pub struct CalendarConfig {
    pub email: String,
    pub app_password: SecretString,
    pub timezone: Tz,
    pub allow_invites: bool,
    pub caldav_url: Url,
}

impl CalendarConfig {
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
        let tz_name = env
            .optional("CALENDAR_TIMEZONE")
            .unwrap_or_else(|| DEFAULT_TIMEZONE.to_string());
        let timezone = match tz_name.parse::<Tz>() {
            Ok(tz) => Some(tz),
            Err(_) => {
                env.errors.push(format!(
                    "CALENDAR_TIMEZONE {tz_name:?} is not an IANA time zone name such as Europe/London"
                ));
                None
            }
        };
        let allow_invites = env.bool("ICLOUD_ALLOW_INVITES", false);
        let raw_url = env
            .optional("CALDAV_URL")
            .unwrap_or_else(|| DEFAULT_CALDAV_URL.to_string());
        let caldav_url = match parse_caldav_url(&raw_url) {
            Ok(u) => Some(u),
            Err(msg) => {
                env.errors.push(msg);
                None
            }
        };
        if env.errors.len() != errors_before {
            return None;
        }
        Some(CalendarConfig {
            email: email?,
            app_password: app_password?,
            timezone: timezone?,
            allow_invites,
            caldav_url: caldav_url?,
        })
    }
}

fn parse_caldav_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("CALDAV_URL is not a valid URL: {e}"))?;
    if url.scheme() == "https" && url.host_str().is_some() {
        return Ok(url);
    }
    if url.scheme() == "http" && is_loopback(&url) {
        return Ok(url);
    }
    Err("CALDAV_URL must use https (http is only allowed for loopback addresses)".into())
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

    fn load(extra: &[(&str, &str)]) -> (Option<CalendarConfig>, Vec<String>) {
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
        let c = CalendarConfig::from_env(&mut env);
        (c, env.errors)
    }

    #[test]
    fn defaults() {
        let (c, errors) = load(&[]);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert_eq!(c.email, "user@example.com");
        assert_eq!(c.app_password.expose_secret(), "abcd-efgh-ijkl-mnop");
        assert_eq!(c.timezone, chrono_tz::Europe::London);
        assert!(!c.allow_invites);
        assert_eq!(c.caldav_url.as_str(), "https://caldav.icloud.com/");
    }

    #[test]
    fn overrides() {
        let (c, errors) = load(&[
            ("CALENDAR_TIMEZONE", "America/New_York"),
            ("ICLOUD_ALLOW_INVITES", "true"),
            ("CALDAV_URL", "http://127.0.0.1:9999"),
        ]);
        assert!(errors.is_empty(), "{errors:?}");
        let c = c.unwrap();
        assert_eq!(c.timezone, chrono_tz::America::New_York);
        assert!(c.allow_invites);
        assert_eq!(c.caldav_url.as_str(), "http://127.0.0.1:9999/");
    }

    #[test]
    fn invalid_timezone_fails_startup() {
        let (c, errors) = load(&[("CALENDAR_TIMEZONE", "Mars/Olympus")]);
        assert!(c.is_none());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("CALENDAR_TIMEZONE") && e.contains("Mars/Olympus")),
            "{errors:?}"
        );
    }

    #[test]
    fn caldav_url_must_be_https_unless_loopback() {
        for ok in [
            "https://caldav.icloud.com",
            "http://localhost:8080",
            "http://[::1]:8080",
        ] {
            assert!(parse_caldav_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://caldav.icloud.com",
            "ftp://x",
            "not a url",
            "http://10.0.0.1",
        ] {
            assert!(parse_caldav_url(bad).is_err(), "{bad}");
        }
        let (c, errors) = load(&[("CALDAV_URL", "http://caldav.icloud.com")]);
        assert!(c.is_none());
        assert!(
            errors.iter().any(|e| e.contains("CALDAV_URL")),
            "{errors:?}"
        );
    }

    #[test]
    fn calendar_errors_are_reported_together() {
        let (c, errors) = load(&[
            ("ICLOUD_EMAIL", "not-an-email"),
            ("CALENDAR_TIMEZONE", "Nowhere/Land"),
            ("CALDAV_URL", "ftp://x"),
        ]);
        assert!(c.is_none());
        let joined = errors.join("\n");
        assert!(
            joined.contains("ICLOUD_EMAIL must be an email address"),
            "{joined}"
        );
        assert!(joined.contains("CALENDAR_TIMEZONE"), "{joined}");
        assert!(joined.contains("CALDAV_URL"), "{joined}");
        assert!(!joined.contains("abcd-efgh"), "secret leaked: {joined}");
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
            CalendarConfig::from_env(&mut env).is_some(),
            "{:?}",
            env.errors
        );
        let read = read.lock().unwrap();
        for k in read.iter() {
            assert!(
                k.starts_with("ICLOUD_") || k.starts_with("CALENDAR_") || k.starts_with("CALDAV_"),
                "calendar config read a server variable: {k}"
            );
        }
        assert!(!read.iter().any(|k| k == "ICLOUD_SERVICES"), "{read:?}");
    }

    #[test]
    fn missing_app_password_is_reported() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("ICLOUD_EMAIL".into(), "user@example.com".into());
        let mut env = EnvReader::new(move |k: &str| map.get(k).cloned());
        assert!(CalendarConfig::from_env(&mut env).is_none());
        assert!(
            env.errors
                .iter()
                .any(|e| e == "ICLOUD_APP_PASSWORD is required"),
            "{:?}",
            env.errors
        );
    }
}
