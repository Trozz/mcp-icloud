use super::AuthState;
use crate::jwt::TokenKind;
use crate::pkce;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use secrecy::ExposeSecret;
use std::sync::Arc;

fn unauthorized(st: &AuthState, token_present: bool) -> Response {
    let mut value = format!(
        "Bearer resource_metadata=\"{}\"",
        st.resource_metadata_url()
    );
    if token_present {
        value.push_str(", error=\"invalid_token\"");
    }
    let mut res = StatusCode::UNAUTHORIZED.into_response();
    if let Ok(v) = HeaderValue::from_str(&value) {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    res
}

pub async fn require_bearer(
    State(st): State<Arc<AuthState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let Some(token) = token else {
        return unauthorized(&st, false);
    };
    if st
        .settings
        .static_token
        .as_ref()
        .is_some_and(|s| pkce::ct_eq(&token, s.expose_secret()))
    {
        return next.run(req).await;
    }
    match st.jwt.verify(&token, TokenKind::Access) {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(_) => unauthorized(&st, true),
    }
}
