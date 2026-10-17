use mcp_icloud_dav::DavError;

/// Tool errors. Messages never contain contact content (names, numbers, addresses, notes), because
/// failed tool calls are logged with their message.
#[derive(Debug, thiserror::Error)]
pub enum ContactsError {
    #[error(
        "iCloud rejected the login. Check ICLOUD_EMAIL and that ICLOUD_APP_PASSWORD is a valid app-specific password (appleid.apple.com > Sign-In and Security > App-Specific Passwords)"
    )]
    Auth,
    #[error("iCloud refused the write to {0} (HTTP 403)")]
    ReadOnly(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("the contact changed since it was read; re-read it and retry")]
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

impl ContactsError {
    pub fn kind(&self) -> &'static str {
        match self {
            ContactsError::Auth => "auth",
            ContactsError::ReadOnly(_) => "read_only",
            ContactsError::NotFound(_) => "not_found",
            ContactsError::Conflict => "conflict",
            ContactsError::Quota => "quota",
            ContactsError::Validation(_) => "validation",
            ContactsError::Upstream(_) => "upstream",
            ContactsError::Internal(_) => "internal",
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        ContactsError::Validation(msg.into())
    }
}

impl From<DavError> for ContactsError {
    fn from(e: DavError) -> Self {
        match e {
            DavError::Auth => ContactsError::Auth,
            DavError::Forbidden(path) => ContactsError::ReadOnly(path),
            DavError::NotFound(path) => ContactsError::NotFound(path),
            DavError::Conflict => ContactsError::Conflict,
            DavError::Quota => ContactsError::Quota,
            DavError::Upstream(msg) => ContactsError::Upstream(msg),
            DavError::Internal(msg) => ContactsError::Internal(msg),
        }
    }
}

pub type Result<T> = std::result::Result<T, ContactsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_the_spec() {
        let all = [
            (ContactsError::Auth, "auth"),
            (ContactsError::ReadOnly("/card/".into()), "read_only"),
            (ContactsError::NotFound("x".into()), "not_found"),
            (ContactsError::Conflict, "conflict"),
            (ContactsError::Quota, "quota"),
            (ContactsError::invalid("bad"), "validation"),
            (ContactsError::Upstream("500".into()), "upstream"),
            (ContactsError::Internal("bug".into()), "internal"),
        ];
        for (e, kind) in all {
            assert_eq!(e.kind(), kind);
        }
    }

    #[test]
    fn dav_errors_map_to_the_same_kinds() {
        let all = [
            (DavError::Auth, "auth"),
            (DavError::Forbidden("/card/x.vcf".into()), "read_only"),
            (DavError::NotFound("/card/x.vcf".into()), "not_found"),
            (DavError::Conflict, "conflict"),
            (DavError::Quota, "quota"),
            (DavError::Upstream("HTTP 500".into()), "upstream"),
            (DavError::Internal("bug".into()), "internal"),
        ];
        for (e, kind) in all {
            assert_eq!(ContactsError::from(e).kind(), kind);
        }
    }

    #[test]
    fn messages_guide_the_caller() {
        assert!(
            ContactsError::Auth
                .to_string()
                .contains("app-specific password")
        );
        assert_eq!(
            ContactsError::Conflict.to_string(),
            "the contact changed since it was read; re-read it and retry"
        );
        assert!(
            ContactsError::from(DavError::Forbidden("/1234567890/carddavhome/card/".into()))
                .to_string()
                .contains("HTTP 403")
        );
    }
}
