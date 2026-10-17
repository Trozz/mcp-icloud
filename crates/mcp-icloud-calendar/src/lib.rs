pub mod caldav;
pub mod config;
pub mod error;
pub mod freebusy;
pub mod ical;
pub mod service;
pub mod tools;
pub mod tz;
pub mod view;

pub use service::{CalendarService, from_env};

pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
