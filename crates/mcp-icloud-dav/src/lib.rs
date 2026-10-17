//! Shared WebDAV layer for the iCloud services: the HTTP client, principal and home discovery,
//! `multistatus` parsing and lossless content lines (iCalendar and vCard).

pub mod client;
pub mod discovery;
pub mod error;
pub mod line;
pub mod xml;

pub use error::{DavError, Result};
