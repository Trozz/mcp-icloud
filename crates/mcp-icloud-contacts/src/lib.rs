pub mod book;
pub mod config;
pub mod error;
pub mod search;
pub mod vcard;

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
