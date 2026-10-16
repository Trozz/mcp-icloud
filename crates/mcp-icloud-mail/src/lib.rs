pub mod cache;
pub mod config;
pub mod error;
pub mod mail;

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
