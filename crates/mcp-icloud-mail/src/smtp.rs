use crate::error::{MailError, Result};
use futures::future::BoxFuture;
use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Mutex;
use std::time::Instant;

/// A message handed to a `Sender`, as recorded by `RecordingSender`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMail {
    pub from: String,
    pub recipients: Vec<String>,
    pub raw: Vec<u8>,
}

/// Sends an already-built RFC 822 message to an explicit SMTP envelope.
pub trait Sender: Send + Sync {
    fn send<'a>(
        &'a self,
        from: &'a str,
        recipients: &'a [String],
        raw: &'a [u8],
    ) -> BoxFuture<'a, Result<()>>;
}

fn parse_address(s: &str) -> Result<Address> {
    s.parse()
        .map_err(|_| MailError::Invalid(format!("not a valid email address: {s}")))
}

fn envelope(from: &str, recipients: &[String]) -> Result<Envelope> {
    if recipients.is_empty() {
        return Err(MailError::Invalid("the message has no recipients".into()));
    }
    let to = recipients
        .iter()
        .map(|r| parse_address(r))
        .collect::<Result<Vec<_>>>()?;
    Envelope::new(Some(parse_address(from)?), to).map_err(|e| MailError::Invalid(e.to_string()))
}

pub struct SmtpSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpSender {
    /// iCloud SMTP: `smtp.mail.me.com:587` with STARTTLS required (rustls, webpki roots).
    /// Must be called inside a tokio runtime (the connection pool spawns a task).
    pub fn icloud(username: &str, password: &SecretString) -> Result<Self> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay("smtp.mail.me.com")
            .map_err(|e| MailError::Smtp(e.to_string()))?
            .credentials(Credentials::new(
                username.to_string(),
                password.expose_secret().to_string(),
            ))
            .build();
        Ok(Self { transport })
    }
}

impl Sender for SmtpSender {
    fn send<'a>(
        &'a self,
        from: &'a str,
        recipients: &'a [String],
        raw: &'a [u8],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let env = envelope(from, recipients)?;
            let started = Instant::now();
            let result = self.transport.send_raw(&env, raw).await;
            let outcome = if result.is_ok() { "ok" } else { "error" };
            metrics::counter!("smtp_sends_total", "outcome" => outcome).increment(1);
            tracing::info!(
                recipients = recipients.len(),
                outcome,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "smtp send"
            );
            result
                .map(|_| ())
                .map_err(|e| MailError::Smtp(e.to_string()))
        })
    }
}

/// Test double: records every message instead of sending it. `fail_with` makes the next send fail.
#[derive(Default)]
pub struct RecordingSender {
    pub sent: Mutex<Vec<SentMail>>,
    pub fail_with: Mutex<Option<String>>,
}

impl Sender for RecordingSender {
    fn send<'a>(
        &'a self,
        from: &'a str,
        recipients: &'a [String],
        raw: &'a [u8],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            envelope(from, recipients)?;
            if let Some(msg) = self.fail_with.lock().expect("sender lock").take() {
                return Err(MailError::Smtp(msg));
            }
            self.sent.lock().expect("sender lock").push(SentMail {
                from: from.to_string(),
                recipients: recipients.to_vec(),
                raw: raw.to_vec(),
            });
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rcpt(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn envelope_requires_valid_addresses_and_recipients() {
        assert!(envelope("me@icloud.com", &rcpt(&["a@example.com", "b@example.com"])).is_ok());
        assert!(matches!(
            envelope("me@icloud.com", &[]),
            Err(MailError::Invalid(_))
        ));
        assert!(matches!(
            envelope("me@icloud.com", &rcpt(&["not an address"])),
            Err(MailError::Invalid(_))
        ));
    }

    // The pooled lettre transport spawns on the tokio runtime, so this must run inside one.
    #[tokio::test]
    async fn icloud_sender_builds_without_connecting() {
        mcp_server_core::install_crypto_provider();
        assert!(SmtpSender::icloud("me@icloud.com", &"pw".to_string().into()).is_ok());
    }

    #[tokio::test]
    async fn recording_sender_records_and_fails_once() {
        let s = RecordingSender::default();
        let to = rcpt(&["a@example.com"]);
        s.send("me@icloud.com", &to, b"raw").await.unwrap();
        *s.fail_with.lock().unwrap() = Some("550 rejected".into());
        assert!(matches!(
            s.send("me@icloud.com", &to, b"raw").await,
            Err(MailError::Smtp(m)) if m == "550 rejected"
        ));
        s.send("me@icloud.com", &to, b"raw2").await.unwrap();
        let sent = s.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].recipients, to);
        assert_eq!(sent[1].raw, b"raw2");
    }
}
