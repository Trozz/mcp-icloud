use super::AuthState;
use crate::jwt::TokenKind;
use crate::pkce;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

#[derive(Debug)]
pub enum Rejection {
    BadClient(&'static str),
    Redirect {
        redirect_uri: String,
        state: Option<String>,
        error: &'static str,
        description: &'static str,
    },
}

#[derive(Debug, Clone)]
pub struct ValidRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub state: Option<String>,
    pub scope: String,
    pub resource: String,
}

pub fn validate(st: &AuthState, p: &AuthorizeParams) -> Result<ValidRequest, Rejection> {
    let client_id = p
        .client_id
        .as_deref()
        .ok_or(Rejection::BadClient("missing client_id"))?;
    let client = st
        .jwt
        .verify(client_id, TokenKind::Client)
        .map_err(|_| Rejection::BadClient("unknown client"))?;
    let redirect_uri = p
        .redirect_uri
        .as_deref()
        .ok_or(Rejection::BadClient("missing redirect_uri"))?;
    let registered = client.payload.redirect_uris.unwrap_or_default();
    if !registered.iter().any(|r| r == redirect_uri) {
        return Err(Rejection::BadClient(
            "redirect_uri is not registered for this client",
        ));
    }
    let redirect = |error, description| Rejection::Redirect {
        redirect_uri: redirect_uri.to_string(),
        state: p.state.clone(),
        error,
        description,
    };
    if p.response_type.as_deref() != Some("code") {
        return Err(redirect(
            "unsupported_response_type",
            "response_type must be code",
        ));
    }
    if p.code_challenge_method.as_deref() != Some("S256") {
        return Err(redirect(
            "invalid_request",
            "code_challenge_method must be S256",
        ));
    }
    let Some(code_challenge) = p.code_challenge.clone().filter(|c| c.len() == 43) else {
        return Err(redirect("invalid_request", "code_challenge is required"));
    };
    let resource = p.resource.clone().unwrap_or_else(|| st.resource());
    if resource != st.resource() {
        return Err(redirect(
            "invalid_target",
            "resource does not match this server",
        ));
    }
    Ok(ValidRequest {
        client_id: client_id.to_string(),
        redirect_uri: redirect_uri.to_string(),
        code_challenge,
        state: p.state.clone(),
        scope: p.scope.clone().unwrap_or_else(|| "mail".into()),
        resource,
    })
}

pub fn binding(v: &ValidRequest) -> String {
    pkce::s256(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        v.client_id,
        v.redirect_uri,
        v.code_challenge,
        v.state.as_deref().unwrap_or(""),
        v.scope,
        v.resource
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::Payload;
    use crate::oauth::OAuthSettings;

    fn st() -> std::sync::Arc<AuthState> {
        AuthState::new(OAuthSettings {
            public_url: "https://mail.example.com".into(),
            signing_key: "0123456789abcdef0123456789abcdef".to_string().into(),
            admin_password: "pw".to_string().into(),
            static_token: None,
        })
    }

    fn client(st: &AuthState) -> String {
        st.jwt
            .issue(
                TokenKind::Client,
                None,
                Payload {
                    redirect_uris: Some(vec!["https://app.example.com/cb".into()]),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    fn params(client_id: &str) -> AuthorizeParams {
        AuthorizeParams {
            response_type: Some("code".into()),
            client_id: Some(client_id.into()),
            redirect_uri: Some("https://app.example.com/cb".into()),
            code_challenge: Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into()),
            code_challenge_method: Some("S256".into()),
            state: Some("xyz".into()),
            scope: None,
            resource: Some("https://mail.example.com/mcp".into()),
        }
    }

    #[test]
    fn accepts_valid_request() {
        let st = st();
        let v = validate(&st, &params(&client(&st))).unwrap();
        assert_eq!(v.redirect_uri, "https://app.example.com/cb");
        assert_eq!(v.scope, "mail");
        assert_eq!(v.resource, "https://mail.example.com/mcp");
    }

    #[test]
    fn missing_resource_defaults_to_mcp_url() {
        let st = st();
        let mut p = params(&client(&st));
        p.resource = None;
        assert_eq!(
            validate(&st, &p).unwrap().resource,
            "https://mail.example.com/mcp"
        );
    }

    #[test]
    fn invalid_client_is_not_redirected() {
        let st = st();
        assert!(matches!(
            validate(&st, &params("garbage")),
            Err(Rejection::BadClient(_))
        ));
    }

    #[test]
    fn unregistered_redirect_uri_is_not_redirected() {
        let st = st();
        let mut p = params(&client(&st));
        p.redirect_uri = Some("https://evil.example.com/cb".into());
        assert!(matches!(validate(&st, &p), Err(Rejection::BadClient(_))));
    }

    #[test]
    fn plain_pkce_is_rejected_with_redirect() {
        let st = st();
        let mut p = params(&client(&st));
        p.code_challenge_method = Some("plain".into());
        assert!(matches!(
            validate(&st, &p),
            Err(Rejection::Redirect {
                error: "invalid_request",
                ..
            })
        ));
    }

    #[test]
    fn missing_challenge_is_rejected_with_redirect() {
        let st = st();
        let mut p = params(&client(&st));
        p.code_challenge = None;
        assert!(matches!(validate(&st, &p), Err(Rejection::Redirect { .. })));
    }

    #[test]
    fn wrong_response_type_is_rejected_with_redirect() {
        let st = st();
        let mut p = params(&client(&st));
        p.response_type = Some("token".into());
        assert!(matches!(
            validate(&st, &p),
            Err(Rejection::Redirect {
                error: "unsupported_response_type",
                ..
            })
        ));
    }

    #[test]
    fn wrong_resource_is_rejected_with_redirect() {
        let st = st();
        let mut p = params(&client(&st));
        p.resource = Some("https://other.example.com/mcp".into());
        assert!(matches!(
            validate(&st, &p),
            Err(Rejection::Redirect {
                error: "invalid_target",
                ..
            })
        ));
    }

    #[test]
    fn binding_changes_with_any_parameter() {
        let st = st();
        let a = validate(&st, &params(&client(&st))).unwrap();
        let mut b = a.clone();
        b.state = Some("other".into());
        assert_ne!(binding(&a), binding(&b));
    }
}
