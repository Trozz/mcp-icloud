#[derive(Debug, thiserror::Error)]
pub enum CalError {
    #[error(
        "iCloud rejected the login. Check ICLOUD_EMAIL and that ICLOUD_APP_PASSWORD is a valid app-specific password (appleid.apple.com > Sign-In and Security > App-Specific Passwords)"
    )]
    Auth,
    #[error("{0} is read-only")]
    ReadOnly(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("the event changed since it was read; re-read it and retry")]
    Conflict,
    #[error("the iCloud storage quota is exceeded")]
    Quota,
    #[error("{0}")]
    Validation(String),
    #[error("iCloud error: {0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

impl CalError {
    pub fn kind(&self) -> &'static str {
        match self {
            CalError::Auth => "auth",
            CalError::ReadOnly(_) => "read_only",
            CalError::NotFound(_) => "not_found",
            CalError::Conflict => "conflict",
            CalError::Quota => "quota",
            CalError::Validation(_) => "validation",
            CalError::Upstream(_) => "upstream",
            CalError::Internal(_) => "internal",
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        CalError::Validation(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, CalError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_the_spec() {
        let all = [
            (CalError::Auth, "auth"),
            (CalError::ReadOnly("Home".into()), "read_only"),
            (CalError::NotFound("x".into()), "not_found"),
            (CalError::Conflict, "conflict"),
            (CalError::Quota, "quota"),
            (CalError::invalid("bad"), "validation"),
            (CalError::Upstream("500".into()), "upstream"),
            (CalError::Internal("bug".into()), "internal"),
        ];
        for (e, kind) in all {
            assert_eq!(e.kind(), kind);
        }
    }

    #[test]
    fn auth_message_mentions_app_specific_password() {
        assert!(CalError::Auth.to_string().contains("app-specific password"));
    }

    #[test]
    fn conflict_tells_the_caller_to_retry() {
        assert!(
            CalError::Conflict
                .to_string()
                .contains("re-read it and retry")
        );
    }
}
