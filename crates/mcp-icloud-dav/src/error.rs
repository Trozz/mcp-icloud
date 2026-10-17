//! Errors from a CalDAV or CardDAV server and from reading its data. Each service maps them to its
//! own tool errors.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavError {
    /// HTTP 401.
    #[error("iCloud rejected the login")]
    Auth,
    /// HTTP 403; the request path.
    #[error("{0} is read-only")]
    Forbidden(String),
    /// HTTP 404; the request path.
    #[error("not found: {0}")]
    NotFound(String),
    /// HTTP 412.
    #[error("precondition failed")]
    Conflict,
    /// HTTP 413 or 507.
    #[error("the iCloud storage quota is exceeded")]
    Quota,
    /// Any other status, a timeout, a transport error, or data that cannot be read.
    #[error("iCloud error: {0}")]
    Upstream(String),
    /// Client construction failures and other bugs.
    #[error("{0}")]
    Internal(String),
}

impl DavError {
    /// The tool error kind the services report for this error.
    pub fn kind(&self) -> &'static str {
        match self {
            DavError::Auth => "auth",
            DavError::Forbidden(_) => "read_only",
            DavError::NotFound(_) => "not_found",
            DavError::Conflict => "conflict",
            DavError::Quota => "quota",
            DavError::Upstream(_) => "upstream",
            DavError::Internal(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, DavError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_the_service_error_kinds() {
        let all = [
            (DavError::Auth, "auth"),
            (DavError::Forbidden("/a/".into()), "read_only"),
            (DavError::NotFound("/a/".into()), "not_found"),
            (DavError::Conflict, "conflict"),
            (DavError::Quota, "quota"),
            (DavError::Upstream("HTTP 500".into()), "upstream"),
            (DavError::Internal("bug".into()), "internal"),
        ];
        for (e, kind) in all {
            assert_eq!(e.kind(), kind, "{e:?}");
        }
    }

    #[test]
    fn messages_name_the_cause() {
        assert_eq!(
            DavError::Forbidden("/1234567890/carddavhome/card/".into()).to_string(),
            "/1234567890/carddavhome/card/ is read-only"
        );
        assert_eq!(
            DavError::Upstream("GET /x returned HTTP 500".into()).to_string(),
            "iCloud error: GET /x returned HTTP 500"
        );
        assert_eq!(DavError::Conflict.to_string(), "precondition failed");
    }
}
