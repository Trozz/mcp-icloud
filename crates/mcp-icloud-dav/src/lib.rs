//! Shared WebDAV layer for the iCloud services: errors and lossless content lines (iCalendar and
//! vCard).

pub mod error;
pub mod line;

pub use error::{DavError, Result};
