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
    /// A name that another calendar already has.
    #[error("{0}")]
    Exists(String),
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
            CalError::Exists(_) => "conflict",
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

impl From<mcp_icloud_dav::DavError> for CalError {
    fn from(e: mcp_icloud_dav::DavError) -> Self {
        use mcp_icloud_dav::DavError;
        match e {
            DavError::Auth => CalError::Auth,
            DavError::Forbidden(path) => CalError::ReadOnly(path),
            DavError::NotFound(path) => CalError::NotFound(path),
            DavError::Conflict => CalError::Conflict,
            DavError::Quota => CalError::Quota,
            DavError::Upstream(msg) => CalError::Upstream(msg),
            DavError::Internal(msg) => CalError::Internal(msg),
        }
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
            (
                CalError::Exists("a calendar named \"Work\" already exists".into()),
                "conflict",
            ),
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
    fn dav_errors_keep_their_kind_and_text() {
        use mcp_icloud_dav::DavError;
        for e in [
            DavError::Auth,
            DavError::Forbidden("/1234567890/calendars/home/".into()),
            DavError::NotFound("/1234567890/calendars/home/x.ics".into()),
            DavError::Conflict,
            DavError::Quota,
            DavError::Upstream("GET /x returned HTTP 500".into()),
            DavError::Internal("bug".into()),
        ] {
            let kind = e.kind();
            assert_eq!(CalError::from(e).kind(), kind);
        }
        assert_eq!(
            CalError::from(DavError::Forbidden("calendar \"Home\"".into())).to_string(),
            "calendar \"Home\" is read-only"
        );
        assert_eq!(
            CalError::from(DavError::Upstream("GET /x returned HTTP 500".into())).to_string(),
            "iCloud error: GET /x returned HTTP 500"
        );
        assert!(
            CalError::from(DavError::Auth)
                .to_string()
                .contains("app-specific password")
        );
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
