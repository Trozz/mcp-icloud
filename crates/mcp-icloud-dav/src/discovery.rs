//! Principal and home-set discovery, shared by CalDAV and CardDAV.

use crate::client::DavClient;
use crate::error::{DavError, Result};
use crate::xml::{CALDAV, CARDDAV, Props, propfind_principal};
use url::Url;

/// Which home set to look up on the principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSet {
    Calendar,
    AddressBook,
}

/// What discovery found: the principal, its home and the principal's properties.
#[derive(Debug, Clone)]
pub struct Home {
    pub principal: Url,
    /// Absolute home URL on the partition host.
    pub home: Url,
    /// Props of the principal response (calendar-user-address-set lands in `address_set`).
    pub principal_props: Props,
}

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;

fn propfind_home_set(kind: HomeSet) -> String {
    match kind {
        HomeSet::Calendar => format!(
            r#"{HEAD}<d:propfind xmlns:d="DAV:" xmlns:c="{CALDAV}"><d:prop><c:calendar-home-set/><c:calendar-user-address-set/></d:prop></d:propfind>"#
        ),
        HomeSet::AddressBook => format!(
            r#"{HEAD}<d:propfind xmlns:d="DAV:" xmlns:card="{CARDDAV}"><d:prop><card:addressbook-home-set/></d:prop></d:propfind>"#
        ),
    }
}

fn home_href(kind: HomeSet, props: &Props) -> Option<&str> {
    match kind {
        HomeSet::Calendar => props.calendar_home_set.as_deref(),
        HomeSet::AddressBook => props.addressbook_home_set.as_deref(),
    }
}

/// `base.join(href)`, as an upstream error when the server sent something unusable.
pub fn join(base: &Url, href: &str) -> Result<Url> {
    base.join(href)
        .map_err(|e| DavError::Upstream(format!("invalid href {href:?} from the server: {e}")))
}

/// From the client's base URL: the current principal, then its calendar or address book home.
pub async fn discover_home(client: &DavClient, kind: HomeSet) -> Result<Home> {
    let base = client.base().clone();
    let (answered, principal) = client.propfind(&base, 0, propfind_principal()).await?;
    let href = principal
        .iter()
        .find_map(|r| r.props.current_user_principal.clone())
        .ok_or_else(|| DavError::Upstream("the server did not name a principal".into()))?;
    let principal_url = join(&answered, &href)?;

    let (answered, found) = client
        .propfind(&principal_url, 0, propfind_home_set(kind))
        .await?;
    let missing = match kind {
        HomeSet::Calendar => "the server did not name a calendar home",
        HomeSet::AddressBook => "the server did not name an address book home",
    };
    let props = found
        .iter()
        .find(|r| home_href(kind, &r.props).is_some())
        .map(|r| r.props.clone())
        .ok_or_else(|| DavError::Upstream(missing.into()))?;
    let home = join(&answered, home_href(kind, &props).unwrap_or_default())?;
    Ok(Home {
        principal: principal_url,
        home,
        principal_props: props,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    use quick_xml::reader::NsReader;

    #[test]
    fn home_set_bodies_are_well_formed_and_ask_for_the_right_property() {
        for (kind, wanted) in [
            (HomeSet::Calendar, "<c:calendar-home-set/>"),
            (HomeSet::AddressBook, "<card:addressbook-home-set/>"),
        ] {
            let body = propfind_home_set(kind);
            assert!(body.contains(wanted), "{body}");
            let mut r = NsReader::from_str(&body);
            loop {
                let (ns, ev) = r.read_resolved_event().expect("well-formed XML");
                if let Event::Start(_) | Event::Empty(_) = ev {
                    assert!(matches!(ns, ResolveResult::Bound(_)), "{body}");
                }
                if let Event::Eof = ev {
                    break;
                }
            }
        }
        assert!(propfind_home_set(HomeSet::Calendar).contains("<c:calendar-user-address-set/>"));
    }

    #[test]
    fn join_resolves_paths_and_keeps_absolute_urls() {
        let base = Url::parse("https://contacts.icloud.com/").unwrap();
        assert_eq!(
            join(&base, "/1234567890/principal/").unwrap().as_str(),
            "https://contacts.icloud.com/1234567890/principal/"
        );
        assert_eq!(
            join(
                &base,
                "https://p99-contacts.icloud.com:443/1234567890/carddavhome/"
            )
            .unwrap()
            .as_str(),
            "https://p99-contacts.icloud.com/1234567890/carddavhome/"
        );
        assert_eq!(join(&base, "http://[::1").unwrap_err().kind(), "upstream");
    }
}
