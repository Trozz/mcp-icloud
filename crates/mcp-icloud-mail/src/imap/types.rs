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
