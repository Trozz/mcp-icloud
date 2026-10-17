pub mod config;
pub mod error;
pub mod ical;
pub mod tz;

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
