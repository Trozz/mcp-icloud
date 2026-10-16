pub mod config;
pub mod guards;
pub mod jwt;
pub mod oauth;
pub mod pkce;
pub mod server;

pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
