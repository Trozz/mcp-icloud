//! Shared WebDAV layer for the iCloud services: errors, `multistatus` parsing and lossless content
//! lines (iCalendar and vCard).

pub mod error;
pub mod line;
pub mod xml;

pub use error::{DavError, Result};
