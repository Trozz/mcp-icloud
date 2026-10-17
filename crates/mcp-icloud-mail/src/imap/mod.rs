mod client;
pub mod proto;
pub mod query;
mod read;
pub mod types;
mod write;

pub use client::{COMMAND_TIMEOUT, CONNECT_TIMEOUT, Connector, ImapClient, ImapSettings};
pub use query::SearchQuery;
pub use types::{Caps, ListedMailbox, MailboxInfo, MessageSummary, Role, SearchPage};
