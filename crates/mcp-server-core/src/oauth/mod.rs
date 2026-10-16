pub mod authorize;
pub mod metadata;
pub mod register;

use crate::guards::{LoginLimiter, ReplayGuard};
use crate::jwt::Jwt;
use axum::Router;
use axum::routing::{get, post};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::time::Duration;

pub struct OAuthSettings {
    pub public_url: String,
    pub signing_key: SecretString,
    pub admin_password: SecretString,
    pub static_token: Option<SecretString>,
}

pub struct AuthState {
    pub settings: OAuthSettings,
    pub jwt: Jwt,
    pub limiter: LoginLimiter,
    pub replay: ReplayGuard,
}

impl AuthState {
    pub fn new(settings: OAuthSettings) -> Arc<Self> {
        let resource = format!("{}/mcp", settings.public_url);
        let jwt = Jwt::new(
            settings.signing_key.expose_secret().as_bytes(),
            settings.public_url.clone(),
            resource,
        );
        Arc::new(Self {
            settings,
            jwt,
            limiter: LoginLimiter::new(5, Duration::from_secs(60)),
            replay: ReplayGuard::new(),
        })
    }

    pub fn resource(&self) -> String {
        format!("{}/mcp", self.settings.public_url)
    }

    pub fn resource_metadata_url(&self) -> String {
        format!(
            "{}/.well-known/oauth-protected-resource",
            self.settings.public_url
        )
    }
}

pub fn oauth_router(state: Arc<AuthState>) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(metadata::protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(metadata::protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(metadata::authorization_server),
        )
        .route("/register", post(register::register))
        .with_state(state)
}
