use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMailbox {
    pub name: String,
    pub role: Option<Role>,
    pub selectable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailboxInfo {
    pub name: String,
    pub role: Option<Role>,
    pub total: u32,
    pub unseen: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    pub move_: bool,
    pub uidplus: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MessageSummary {
    pub uid: u32,
    pub date: Option<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: String,
    pub message_id: Option<String>,
    pub flags: Vec<String>,
    pub size: u32,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub mailbox: String,
    pub uidvalidity: u32,
    pub messages: Vec<MessageSummary>,
    pub next_cursor: Option<String>,
}
