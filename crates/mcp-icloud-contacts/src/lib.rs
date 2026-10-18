pub mod book;
pub mod config;
pub mod duplicates;
pub mod error;
pub mod index;
pub mod search;
pub mod service;
pub mod tools;
pub mod vcard;

pub use service::{ContactsService, from_env};

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
