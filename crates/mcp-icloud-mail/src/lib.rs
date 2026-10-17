pub mod cache;
pub mod config;
pub mod error;
pub mod imap;
pub mod mail;
pub mod service;
pub mod smtp;
pub mod tools;

pub use service::{MailService, from_env};

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
