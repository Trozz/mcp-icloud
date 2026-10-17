//! Thin CalDAV-over-HTTP client: Basic auth scoped to trusted origins, manual redirects,
//! timeouts, one retry for safe methods, status mapping and metrics.

use crate::caldav::xml::{DavResponse, parse_multistatus};
use crate::error::{CalError, Result};
use mcp_server_core::observability::content_logging_enabled;
use reqwest::header::{CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, LOCATION};
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use std::time::{Duration, Instant};
use url::Url;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_REDIRECTS: usize = 5;

pub struct CaldavSettings {
    /// `CALDAV_URL`: where discovery starts. Also decides which origins are trusted.
    pub base: Url,
    pub username: String,
    pub password: SecretString,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// Always log request and response bodies at debug level. Without it, bodies are logged only
    /// while `DEBUG_UNSAFE_LOG_CONTENT` is on (read per request, because the service is built
    /// before logging is initialised).
    pub log_bodies: bool,
}

impl CaldavSettings {
    pub fn new(base: Url, username: String, password: SecretString) -> Self {
        CaldavSettings {
            base,
            username,
            password,
            connect_timeout: CONNECT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            log_bodies: false,
        }
    }
}

/// Condition for a write: replace exactly this version, or create only if absent.
#[derive(Debug, Clone, PartialEq)]
pub enum Precondition {
    IfMatch(String),
    IfNoneMatch,
}

/// A fetched resource: the URL it was finally read from, its ETag and body.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub url: Url,
    pub etag: Option<String>,
    pub body: String,
}

struct Raw {
    url: Url,
    etag: Option<String>,
    body: String,
}

pub struct CaldavClient {
    http: reqwest::Client,
    settings: CaldavSettings,
}

fn is_icloud_host(host: &str) -> bool {
    host == "icloud.com" || host.ends_with(".icloud.com")
}

fn status_class(status: Option<StatusCode>) -> &'static str {
    match status.map(|s| s.as_u16()) {
        Some(200..=299) => "2xx",
        Some(300..=399) => "3xx",
        Some(400..=499) => "4xx",
        Some(500..=599) => "5xx",
        _ => "error",
    }
}

fn map_status(method: &Method, url: &Url, status: StatusCode) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let path = url.path().to_string();
    Err(match status.as_u16() {
        401 => CalError::Auth,
        403 => CalError::ReadOnly(path),
        404 => CalError::NotFound(path),
        412 => CalError::Conflict,
        413 | 507 => CalError::Quota,
        code => CalError::Upstream(format!("{method} {path} returned HTTP {code}")),
    })
}

impl CaldavClient {
    pub fn new(settings: CaldavSettings) -> Result<Self> {
        mcp_server_core::install_crypto_provider();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(settings.connect_timeout)
            .timeout(settings.request_timeout)
            .user_agent(format!("mcp-icloud-calendar/{}", crate::VERSION))
            .build()
            .map_err(|e| CalError::Internal(format!("building the HTTP client failed: {e}")))?;
        Ok(CaldavClient { http, settings })
    }

    pub fn base(&self) -> &Url {
        &self.settings.base
    }

    fn log_bodies(&self) -> bool {
        self.settings.log_bodies || content_logging_enabled()
    }

    /// Whether a request (and the credentials) may go to `url`. With an iCloud base URL that is
    /// any https `*.icloud.com` host on port 443; otherwise exactly the base URL's origin.
    pub fn is_trusted(&self, url: &Url) -> bool {
        let base = &self.settings.base;
        if base.scheme() == "https" && base.host_str().is_some_and(is_icloud_host) {
            return url.scheme() == "https"
                && url.host_str().is_some_and(is_icloud_host)
                && url.port_or_known_default() == Some(443)
                && url.username().is_empty()
                && url.password().is_none();
        }
        url.scheme() == base.scheme()
            && url.host_str() == base.host_str()
            && url.port_or_known_default() == base.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none()
    }

    /// PROPFIND; returns the URL that answered (after redirects) and the parsed responses.
    pub async fn propfind(
        &self,
        url: &Url,
        depth: u8,
        body: String,
    ) -> Result<(Url, Vec<DavResponse>)> {
        let method = Method::from_bytes(b"PROPFIND").expect("valid method");
        let mut headers = HeaderMap::new();
        headers.insert("Depth", HeaderValue::from(u16::from(depth)));
        let raw = self.send(method, url, headers, Some(body), true).await?;
        Ok((raw.url, parse_multistatus(&raw.body)?))
    }

    /// REPORT with `Depth: 1` (calendar-query).
    pub async fn report(&self, url: &Url, body: String) -> Result<Vec<DavResponse>> {
        let method = Method::from_bytes(b"REPORT").expect("valid method");
        let mut headers = HeaderMap::new();
        headers.insert("Depth", HeaderValue::from_static("1"));
        let raw = self.send(method, url, headers, Some(body), true).await?;
        parse_multistatus(&raw.body)
    }

    pub async fn get(&self, url: &Url) -> Result<Fetched> {
        let raw = self
            .send(Method::GET, url, HeaderMap::new(), None, true)
            .await?;
        Ok(Fetched {
            url: raw.url,
            etag: raw.etag,
            body: raw.body,
        })
    }

    /// PUT an iCalendar resource; returns the new ETag when the server sends one.
    pub async fn put(&self, url: &Url, body: String, pre: Precondition) -> Result<Option<String>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/calendar; charset=utf-8"),
        );
        match pre {
            Precondition::IfMatch(etag) => {
                let v = HeaderValue::from_str(&etag)
                    .map_err(|_| CalError::Upstream("invalid ETag from server".into()))?;
                headers.insert("If-Match", v);
            }
            Precondition::IfNoneMatch => {
                headers.insert("If-None-Match", HeaderValue::from_static("*"));
            }
        }
        let raw = self
            .send(Method::PUT, url, headers, Some(body), false)
            .await?;
        Ok(raw.etag)
    }

    /// DELETE exactly the version `etag`.
    pub async fn delete(&self, url: &Url, etag: &str) -> Result<()> {
        let mut headers = HeaderMap::new();
        let v = HeaderValue::from_str(etag)
            .map_err(|_| CalError::Upstream("invalid ETag from server".into()))?;
        headers.insert("If-Match", v);
        self.send(Method::DELETE, url, headers, None, false).await?;
        Ok(())
    }

    async fn send(
        &self,
        method: Method,
        url: &Url,
        headers: HeaderMap,
        body: Option<String>,
        retry: bool,
    ) -> Result<Raw> {
        let mut url = url.clone();
        let mut hops = 0;
        let mut retried = false;
        loop {
            if !self.is_trusted(&url) {
                return Err(CalError::Upstream(format!(
                    "refusing to contact {} (not a trusted CalDAV origin)",
                    url.origin().ascii_serialization()
                )));
            }
            let (status, outcome) = self.attempt(&method, &url, &headers, &body).await;
            let response = match outcome {
                Ok(r) => r,
                Err(e) if retry && !retried => {
                    tracing::debug!(method = %method, path = url.path(), error = %e, "retrying");
                    retried = true;
                    continue;
                }
                Err(e) => {
                    return Err(CalError::Upstream(format!(
                        "{method} {} failed: {e}",
                        url.path()
                    )));
                }
            };
            let status = status.expect("status present on a response");
            // 507 (quota) is not transient, so it is not retried.
            if status.is_server_error()
                && status != StatusCode::INSUFFICIENT_STORAGE
                && retry
                && !retried
            {
                tracing::debug!(method = %method, path = url.path(), status = status.as_u16(), "retrying");
                retried = true;
                continue;
            }
            if status.is_redirection()
                && let Some(location) = response.headers().get(LOCATION)
            {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(CalError::Upstream(format!(
                        "too many redirects from {}",
                        url.path()
                    )));
                }
                let location = location
                    .to_str()
                    .ok()
                    .and_then(|l| url.join(l).ok())
                    .ok_or_else(|| CalError::Upstream("invalid redirect location".into()))?;
                tracing::debug!(from = url.path(), to = %location.origin().ascii_serialization(), "redirect");
                url = location;
                continue;
            }
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let text = response.text().await.map_err(|e| {
                CalError::Upstream(format!("reading {method} {} failed: {e}", url.path()))
            })?;
            if self.log_bodies() {
                tracing::debug!(method = %method, path = url.path(), status = status.as_u16(), body = %text, "caldav response");
            }
            map_status(&method, &url, status)?;
            return Ok(Raw {
                url,
                etag,
                body: text,
            });
        }
    }

    async fn attempt(
        &self,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        body: &Option<String>,
    ) -> (
        Option<StatusCode>,
        std::result::Result<reqwest::Response, reqwest::Error>,
    ) {
        let mut req = self
            .http
            .request(method.clone(), url.clone())
            .headers(headers.clone())
            .basic_auth(
                &self.settings.username,
                Some(self.settings.password.expose_secret()),
            );
        if let Some(b) = body {
            if self.log_bodies() {
                tracing::debug!(method = %method, path = url.path(), body = %b, "caldav request");
            }
            if !headers.contains_key(CONTENT_TYPE) {
                req = req.header(CONTENT_TYPE, "application/xml; charset=utf-8");
            }
            req = req.body(b.clone());
        }
        let started = Instant::now();
        let result = req.send().await;
        let status = result.as_ref().ok().map(|r| r.status());
        let class = status_class(status);
        metrics::counter!("caldav_requests_total", "method" => method.to_string(), "status_class" => class)
            .increment(1);
        metrics::histogram!("caldav_request_duration_seconds", "method" => method.to_string())
            .record(started.elapsed().as_secs_f64());
        tracing::debug!(
            method = %method,
            path = url.path(),
            status = status.map(|s| s.as_u16()),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "caldav request"
        );
        (status, result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn client(base: &str) -> CaldavClient {
        CaldavClient::new(CaldavSettings::new(
            Url::parse(base).unwrap(),
            "user@example.com".into(),
            SecretString::from("app-pass"),
        ))
        .unwrap()
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn icloud_base_trusts_any_icloud_host_on_https_443_only() {
        let c = client("https://caldav.icloud.com");
        assert!(c.is_trusted(&url("https://caldav.icloud.com/")));
        assert!(c.is_trusted(&url(
            "https://p99-caldav.icloud.com:443/1234567890/calendars/"
        )));
        assert!(!c.is_trusted(&url("http://p99-caldav.icloud.com/")));
        assert!(!c.is_trusted(&url("https://p99-caldav.icloud.com:8443/")));
        assert!(!c.is_trusted(&url("https://icloud.com.evil.example/")));
        assert!(!c.is_trusted(&url("https://evilicloud.com/")));
        assert!(!c.is_trusted(&url("https://user:pw@p99-caldav.icloud.com/")));
    }

    #[test]
    fn other_base_trusts_exactly_its_origin() {
        let c = client("http://127.0.0.1:8080/");
        assert!(c.is_trusted(&url("http://127.0.0.1:8080/1234567890/calendars/")));
        assert!(!c.is_trusted(&url("http://127.0.0.1:8081/")));
        assert!(!c.is_trusted(&url("https://127.0.0.1:8080/")));
        assert!(!c.is_trusted(&url("http://localhost:8080/")));
        assert!(!c.is_trusted(&url("https://caldav.icloud.com/")));
    }

    #[test]
    fn statuses_map_to_the_spec_error_kinds() {
        let u = url("https://p99-caldav.icloud.com/1234567890/calendars/home/x.ics");
        let kind = |code: u16| {
            map_status(&Method::PUT, &u, StatusCode::from_u16(code).unwrap())
                .err()
                .map(|e| e.kind())
        };
        assert_eq!(kind(200), None);
        assert_eq!(kind(201), None);
        assert_eq!(kind(204), None);
        assert_eq!(kind(207), None);
        assert_eq!(kind(401), Some("auth"));
        assert_eq!(kind(403), Some("read_only"));
        assert_eq!(kind(404), Some("not_found"));
        assert_eq!(kind(412), Some("conflict"));
        assert_eq!(kind(413), Some("quota"));
        assert_eq!(kind(507), Some("quota"));
        assert_eq!(kind(400), Some("upstream"));
        assert_eq!(kind(503), Some("upstream"));
    }

    #[test]
    fn status_classes() {
        assert_eq!(status_class(Some(StatusCode::MULTI_STATUS)), "2xx");
        assert_eq!(status_class(Some(StatusCode::MOVED_PERMANENTLY)), "3xx");
        assert_eq!(status_class(Some(StatusCode::PRECONDITION_FAILED)), "4xx");
        assert_eq!(status_class(Some(StatusCode::BAD_GATEWAY)), "5xx");
        assert_eq!(status_class(None), "error");
    }
}
