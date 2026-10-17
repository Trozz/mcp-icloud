//! What the tools show and accept. Field names match the spec; empty fields are left out of the
//! JSON the tools return.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
}

/// An email address, phone number or URL with its label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Entry {
    /// The address, number or URL.
    pub value: String,
    /// A plain label: home, work, other, mobile, iPhone, main, pager, home fax, work fax, home page, or any custom text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// true for the preferred entry; at most one per list.
    #[serde(default, skip_serializing_if = "is_false")]
    pub preferred: bool,
}

/// A postal address with its label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Address {
    /// Street lines, separated by newlines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// State, county or province.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postcode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// home, work, other, or any custom text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// true for the preferred address; at most one.
    #[serde(default, skip_serializing_if = "is_false")]
    pub preferred: bool,
}

/// A date such as an anniversary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DateEntry {
    /// YYYY-MM-DD, or --MM-DD without a year.
    pub date: String,
    /// anniversary, other (the default), or any custom text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A related person by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Related {
    pub name: String,
    /// mother, father, parent, brother, sister, sibling, child, son, daughter, spouse, husband, wife, partner, friend, assistant, manager, other (the default), or any custom text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A group a contact belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupRef {
    pub id: String,
    pub name: String,
}

/// A contact as the tools show it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Contact {
    pub id: String,
    pub is_me: bool,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub middle_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_title: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub emails: Vec<Entry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub phones: Vec<Entry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<Entry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dates: Vec<DateEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub social: Vec<Entry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub im: Vec<Entry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupRef>,
    pub has_photo: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

/// A group (an iOS List): its name and the contact UIDs it lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
}

/// The fields of `create_contact` and `update_contact`. A scalar given as "" is cleared; a list
/// replaces the whole list ([] clears it); fields that are not given are left alone.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ContactFields {
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub middle_name: Option<String>,
    /// Name prefix such as Dr.
    pub prefix: Option<String>,
    /// Name suffix such as Jr.
    pub suffix: Option<String>,
    /// The name shown for the contact. Default: the name parts joined, else the organization.
    pub display_name: Option<String>,
    pub nickname: Option<String>,
    pub organization: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    /// Replaces all email addresses.
    pub emails: Option<Vec<Entry>>,
    /// Replaces all phone numbers.
    pub phones: Option<Vec<Entry>>,
    /// Replaces all URLs.
    pub urls: Option<Vec<Entry>>,
    /// Replaces all postal addresses.
    pub addresses: Option<Vec<Address>>,
    /// Replaces all dates (anniversaries and others).
    pub dates: Option<Vec<DateEntry>>,
    /// Replaces all related names.
    pub related: Option<Vec<Related>>,
    /// YYYY-MM-DD, or --MM-DD without a year.
    pub birthday: Option<String>,
    pub note: Option<String>,
}

impl ContactFields {
    /// true when no field is given.
    pub fn is_empty(&self) -> bool {
        let scalars = [
            &self.given_name,
            &self.family_name,
            &self.middle_name,
            &self.prefix,
            &self.suffix,
            &self.display_name,
            &self.nickname,
            &self.organization,
            &self.department,
            &self.job_title,
            &self.birthday,
            &self.note,
        ];
        scalars.iter().all(|s| s.is_none())
            && self.emails.is_none()
            && self.phones.is_none()
            && self.urls.is_none()
            && self.addresses.is_none()
            && self.dates.is_none()
            && self.related.is_none()
    }

    /// true when any part of the structured name is given.
    pub fn changes_name(&self) -> bool {
        [
            &self.given_name,
            &self.family_name,
            &self.middle_name,
            &self.prefix,
            &self.suffix,
        ]
        .iter()
        .any(|s| s.is_some())
    }
}
