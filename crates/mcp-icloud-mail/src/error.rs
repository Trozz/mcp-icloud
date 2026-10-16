#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error(
        "iCloud rejected the login. Check ICLOUD_USERNAME and that ICLOUD_APP_PASSWORD is a valid app-specific password (appleid.apple.com > Sign-In and Security > App-Specific Passwords)"
    )]
    AuthFailed,
    #[error("connection to the mail server failed: {0}")]
    Connection(String),
    #[error("mailbox not found: {0}")]
    MailboxNotFound(String),
    #[error("message not found: uid {0}")]
    MessageNotFound(u32),
    #[error(
        "UIDVALIDITY of {mailbox} changed (expected {expected}, now {actual}); re-run search_messages to get fresh uids"
    )]
    UidValidityChanged {
        mailbox: String,
        expected: u32,
        actual: u32,
    },
    #[error("the server rejected {command}: {info}")]
    Rejected { command: String, info: String },
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Invalid(String),
    #[error("sending failed: {0}")]
    Smtp(String),
}

impl MailError {
    pub fn is_connection(&self) -> bool {
        matches!(self, MailError::Connection(_))
    }

    pub fn rejected(command: &str, info: impl Into<String>) -> Self {
        MailError::Rejected {
            command: command.to_string(),
            info: info.into(),
        }
    }
}

impl From<async_imap::error::Error> for MailError {
    fn from(e: async_imap::error::Error) -> Self {
        use async_imap::error::Error;
        match e {
            Error::Io(e) => MailError::Connection(e.to_string()),
            Error::ConnectionLost => MailError::Connection("connection lost".into()),
            Error::No(info) => MailError::rejected("command", info),
            Error::Bad(info) => MailError::rejected("command", info),
            Error::Validate(v) => MailError::Invalid(v.to_string()),
            other => MailError::rejected("command", other.to_string()),
        }
    }
}

impl From<std::io::Error> for MailError {
    fn from(e: std::io::Error) -> Self {
        MailError::Connection(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, MailError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_are_connection_errors() {
        let e: MailError = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe").into();
        assert!(e.is_connection());
    }

    #[test]
    fn imap_no_is_not_a_connection_error() {
        let e: MailError = async_imap::error::Error::No("nope".into()).into();
        assert!(!e.is_connection());
        assert_eq!(e.to_string(), "the server rejected command: nope");
    }

    #[test]
    fn auth_message_mentions_app_specific_password() {
        assert!(
            MailError::AuthFailed
                .to_string()
                .contains("app-specific password")
        );
    }
}
