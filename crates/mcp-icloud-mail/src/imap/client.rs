use super::proto::{ImapSession, ImapStream};
use super::types::{Caps, ListedMailbox, MailboxInfo, Role};
use crate::cache::MessageCache;
use crate::error::{MailError, Result};
use async_imap::types::NameAttribute;
use futures::TryStreamExt;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone)]
pub enum Connector {
    Tls {
        host: String,
        port: u16,
    },
    /// Plaintext TCP, used only by tests against the fake server. Not reachable from config.
    #[doc(hidden)]
    Plain {
        addr: String,
    },
}

#[derive(Clone)]
pub struct ImapSettings {
    pub connector: Connector,
    pub username: String,
    pub password: SecretString,
    pub idle_timeout: Duration,
}

impl ImapSettings {
    pub fn icloud(username: String, password: SecretString, idle_timeout: Duration) -> Self {
        Self {
            connector: Connector::Tls {
                host: "imap.mail.me.com".into(),
                port: 993,
            },
            username,
            password,
            idle_timeout,
        }
    }
}

pub(crate) struct Conn {
    pub session: ImapSession,
    pub caps: Caps,
    pub mailboxes: Vec<ListedMailbox>,
    last_used: Instant,
}

pub struct ImapClient {
    pub(crate) settings: ImapSettings,
    #[allow(dead_code)] // read by fetch_raw in Task 8
    pub(crate) cache: Arc<dyn MessageCache>,
    inner: Mutex<Option<Conn>>,
}

/// Runs `$body` (an expression producing a future, with `$c: &mut Conn` in scope) on the shared
/// connection: connects lazily, reconnects after idle timeout, drops the connection on connection
/// errors and retries once when `$retry` is true (read-only operations only), and records metrics.
macro_rules! with_conn {
    ($self:ident, $label:expr, $retry:expr, |$c:ident| $body:expr) => {{
        let started = std::time::Instant::now();
        let mut guard = $self.inner.lock().await;
        let mut attempt = 0u8;
        let result = loop {
            attempt += 1;
            let $c = match $self.ensure(&mut guard).await {
                Ok(c) => c,
                Err(e) => break Err(e),
            };
            match $body.await {
                Err(e) if e.is_connection() => {
                    *guard = None;
                    metrics::counter!("imap_reconnects_total").increment(1);
                    tracing::warn!(command = $label, error = %e, "imap connection error");
                    if $retry && attempt < 2 {
                        continue;
                    }
                    break Err(e);
                }
                other => {
                    if let Some(c) = guard.as_mut() {
                        c.touch();
                    }
                    break other;
                }
            }
        };
        let outcome = if result.is_ok() { "ok" } else { "error" };
        metrics::counter!("imap_commands_total", "command" => $label, "outcome" => outcome).increment(1);
        metrics::histogram!("imap_command_duration_seconds", "command" => $label).record(started.elapsed().as_secs_f64());
        tracing::debug!(command = $label, outcome, elapsed_ms = started.elapsed().as_millis() as u64, "imap");
        result
    }};
}
#[allow(unused_imports)]
pub(crate) use with_conn;

impl Conn {
    pub(crate) fn touch(&mut self) {
        self.last_used = Instant::now();
    }

    pub fn role(&self, role: Role) -> Option<&str> {
        self.mailboxes
            .iter()
            .find(|m| m.role == Some(role))
            .map(|m| m.name.as_str())
    }
}

pub(crate) fn role_for(name: &str, attrs: &[NameAttribute<'_>]) -> Option<Role> {
    for a in attrs {
        let role = match a {
            NameAttribute::Sent => Role::Sent,
            NameAttribute::Drafts => Role::Drafts,
            NameAttribute::Trash => Role::Trash,
            NameAttribute::Junk => Role::Junk,
            NameAttribute::Archive => Role::Archive,
            _ => continue,
        };
        return Some(role);
    }
    match name.to_ascii_lowercase().as_str() {
        "inbox" => Some(Role::Inbox),
        "sent messages" | "sent" => Some(Role::Sent),
        "drafts" => Some(Role::Drafts),
        "deleted messages" | "trash" => Some(Role::Trash),
        "junk" | "spam" => Some(Role::Junk),
        "archive" => Some(Role::Archive),
        _ => None,
    }
}

pub(crate) async fn list_mailboxes(s: &mut ImapSession) -> Result<Vec<ListedMailbox>> {
    let names: Vec<_> = s.list(Some(""), Some("*")).await?.try_collect().await?;
    if names.is_empty() {
        return Err(MailError::rejected(
            "LIST",
            "the server returned no mailboxes",
        ));
    }
    Ok(names
        .iter()
        .map(|n| ListedMailbox {
            name: n.name().to_string(),
            role: role_for(n.name(), n.attributes()),
            selectable: !n
                .attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::NoSelect)),
        })
        .collect())
}

/// SELECTs a mailbox and returns its UIDVALIDITY, failing if it differs from `expected`.
pub(crate) async fn select(c: &mut Conn, mailbox: &str, expected: Option<u32>) -> Result<u32> {
    let mb = c.session.select(mailbox).await.map_err(|e| match e {
        async_imap::error::Error::No(_) => MailError::MailboxNotFound(mailbox.to_string()),
        other => other.into(),
    })?;
    // A real server always sends UIDVALIDITY on SELECT; its absence means the response was cut
    // short (async-imap does not report a connection dropped mid-response).
    let actual = mb.uid_validity.ok_or_else(|| {
        MailError::Connection(format!("SELECT {mailbox} returned no UIDVALIDITY"))
    })?;
    if let Some(expected) = expected
        && expected != actual
    {
        return Err(MailError::UidValidityChanged {
            mailbox: mailbox.to_string(),
            expected,
            actual,
        });
    }
    Ok(actual)
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

impl ImapClient {
    pub fn new(settings: ImapSettings, cache: Arc<dyn MessageCache>) -> Self {
        Self {
            settings,
            cache,
            inner: Mutex::new(None),
        }
    }

    async fn open_stream(&self) -> Result<Box<dyn ImapStream>> {
        match &self.settings.connector {
            Connector::Tls { host, port } => {
                let tcp = tokio::net::TcpStream::connect((host.as_str(), *port)).await?;
                let name = rustls::pki_types::ServerName::try_from(host.clone())
                    .map_err(|e| MailError::Connection(format!("invalid host {host}: {e}")))?;
                let tls = tokio_rustls::TlsConnector::from(tls_config())
                    .connect(name, tcp)
                    .await?;
                Ok(Box::new(tls))
            }
            Connector::Plain { addr } => Ok(Box::new(
                tokio::net::TcpStream::connect(addr.as_str()).await?,
            )),
        }
    }

    async fn connect(&self) -> Result<Conn> {
        let stream = self.open_stream().await?;
        let mut client = async_imap::Client::new(stream);
        client
            .read_response()
            .await?
            .ok_or_else(|| MailError::Connection("no greeting from server".into()))?;
        let mut session = client
            .login(
                &self.settings.username,
                self.settings.password.expose_secret(),
            )
            .await
            .map_err(|(e, _)| match e {
                async_imap::error::Error::No(_) => MailError::AuthFailed,
                other => other.into(),
            })?;
        let caps = session.capabilities().await?;
        let caps = Caps {
            move_: caps.has_str("MOVE"),
            uidplus: caps.has_str("UIDPLUS"),
        };
        let mailboxes = list_mailboxes(&mut session).await?;
        tracing::info!(
            move_ = caps.move_,
            uidplus = caps.uidplus,
            mailboxes = mailboxes.len(),
            "imap connected"
        );
        Ok(Conn {
            session,
            caps,
            mailboxes,
            last_used: Instant::now(),
        })
    }

    async fn ensure<'g>(
        &self,
        guard: &'g mut MutexGuard<'_, Option<Conn>>,
    ) -> Result<&'g mut Conn> {
        if guard
            .as_ref()
            .is_some_and(|c| c.last_used.elapsed() > self.settings.idle_timeout)
        {
            tracing::debug!("imap connection idle, reconnecting");
            **guard = None;
        }
        if guard.is_none() {
            **guard = Some(self.connect().await?);
        }
        Ok(guard.as_mut().expect("connected above"))
    }

    pub async fn capabilities(&self) -> Result<Caps> {
        with_conn!(self, "CAPABILITY", true, |c| async {
            Ok::<_, MailError>(c.caps)
        })
    }

    /// Name of the mailbox with the given special-use role.
    pub async fn role_mailbox(&self, role: Role) -> Result<String> {
        with_conn!(self, "LIST", true, |c| async {
            c.role(role)
                .map(str::to_string)
                .ok_or_else(|| MailError::MailboxNotFound(format!("{role:?} mailbox")))
        })
    }

    /// All selectable mailboxes with message and unseen counts.
    pub async fn mailboxes(&self) -> Result<Vec<MailboxInfo>> {
        with_conn!(self, "STATUS", true, |c| async {
            c.mailboxes = list_mailboxes(&mut c.session).await?;
            let listed: Vec<ListedMailbox> = c
                .mailboxes
                .iter()
                .filter(|m| m.selectable)
                .cloned()
                .collect();
            let mut out = Vec::with_capacity(listed.len());
            for m in listed {
                let st = c.session.status(&m.name, "(MESSAGES UNSEEN)").await?;
                out.push(MailboxInfo {
                    name: m.name,
                    role: m.role,
                    total: st.exists,
                    unseen: st.unseen.unwrap_or(0),
                });
            }
            Ok::<_, MailError>(out)
        })
    }

    /// SELECTs `mailbox` and returns its UIDVALIDITY (checked against `expected` when given).
    pub async fn uidvalidity(&self, mailbox: &str, expected: Option<u32>) -> Result<u32> {
        with_conn!(self, "SELECT", true, |c| select(c, mailbox, expected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_from_attributes_and_names() {
        assert_eq!(
            role_for("Whatever", &[NameAttribute::Sent]),
            Some(Role::Sent)
        );
        assert_eq!(role_for("INBOX", &[]), Some(Role::Inbox));
        assert_eq!(role_for("Deleted Messages", &[]), Some(Role::Trash));
        assert_eq!(role_for("Sent Messages", &[]), Some(Role::Sent));
        assert_eq!(role_for("Projects", &[]), None);
    }
}
