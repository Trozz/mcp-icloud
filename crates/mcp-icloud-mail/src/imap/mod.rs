mod client;
pub mod proto;
pub mod types;

pub use client::{Connector, ImapClient, ImapSettings};
pub use types::{Caps, ListedMailbox, MailboxInfo, Role};
