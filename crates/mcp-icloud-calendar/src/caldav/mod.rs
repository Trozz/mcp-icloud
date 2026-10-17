pub mod discovery;
pub mod xml;

use mcp_icloud_dav::client::{DavSettings, Protocol};
use secrecy::SecretString;
use url::Url;

/// Client settings for the calendar: CalDAV with the calendar's user agent.
pub fn caldav_settings(base: Url, username: String, password: SecretString) -> DavSettings {
    DavSettings::new(
        Protocol::CalDav,
        base,
        username,
        password,
        format!("mcp-icloud-calendar/{}", crate::VERSION),
    )
}
