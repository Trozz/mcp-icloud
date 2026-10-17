//! WebDAV request bodies shared by CalDAV and CardDAV, and `multistatus` parsing.
//!
//! Elements are matched by namespace plus local name (iCloud declares namespaces per element).
//! Only properties from a `propstat` with a 2xx status are kept, so the 404 blocks iCloud sends
//! for unsupported properties are ignored.

use crate::error::{DavError, Result};
use quick_xml::XmlVersion;
use quick_xml::escape::escape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
pub const CALSERVER: &str = "http://calendarserver.org/ns/";
pub const APPLE: &str = "http://apple.com/ns/ical/";

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;

pub fn propfind_principal() -> String {
    format!(
        r#"{HEAD}<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#
    )
}

/// `sync-collection` (RFC 6578) for the etag of every member: all of them without a token, the
/// changes since `token` with one.
pub fn sync_collection(token: Option<&str>) -> String {
    let token = match token {
        Some(t) => format!("<d:sync-token>{}</d:sync-token>", escape(t)),
        None => "<d:sync-token/>".to_string(),
    };
    format!(
        r#"{HEAD}<d:sync-collection xmlns:d="DAV:">{token}<d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#
    )
}

/// `addressbook-multiget` (RFC 6352) for the etag and vCard of each href.
pub fn addressbook_multiget(hrefs: &[String]) -> String {
    let hrefs: String = hrefs
        .iter()
        .map(|h| format!("<d:href>{}</d:href>", escape(h.as_str())))
        .collect();
    format!(
        r#"{HEAD}<card:addressbook-multiget xmlns:d="DAV:" xmlns:card="{CARDDAV}"><d:prop><d:getetag/><card:address-data/></d:prop>{hrefs}</card:addressbook-multiget>"#
    )
}

/// The properties we read, from every 2xx `propstat` of one `response`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props {
    pub current_user_principal: Option<String>,
    pub calendar_home_set: Option<String>,
    pub addressbook_home_set: Option<String>,
    pub address_set: Vec<String>,
    pub displayname: Option<String>,
    /// Resource types in Clark notation, e.g. `{urn:ietf:params:xml:ns:caldav}calendar`.
    pub resource_types: Vec<String>,
    /// Supported calendar components, e.g. `VEVENT`.
    pub components: Vec<String>,
    /// Privilege local names, e.g. `write`, `write-content`.
    pub privileges: Vec<String>,
    pub color: Option<String>,
    pub ctag: Option<String>,
    pub etag: Option<String>,
    pub calendar_data: Option<String>,
    pub address_data: Option<String>,
    /// A `sync-token` returned as a property (PROPFIND).
    pub sync_token: Option<String>,
    pub max_resource_size: Option<u64>,
    /// The href of the owner's card (`{http://calendarserver.org/ns/}me-card`).
    pub me_card: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DavResponse {
    pub href: String,
    /// Status of the whole response (`<response><status>`), when present.
    pub status: Option<u16>,
    pub props: Props,
}

impl DavResponse {
    pub fn has_type(&self, ns: &str, local: &str) -> bool {
        self.props.resource_types.contains(&clark(ns, local))
    }
}

/// A parsed `multistatus` body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Multistatus {
    pub responses: Vec<DavResponse>,
    /// The `<sync-token>` that is a direct child of `<multistatus>` (sync-collection reports).
    pub sync_token: Option<String>,
}

fn clark(ns: &str, local: &str) -> String {
    format!("{{{ns}}}{local}")
}

/// "HTTP/1.1 404 Not Found" -> 404.
fn status_code(s: &str) -> Option<u16> {
    s.split_whitespace().nth(1)?.parse().ok()
}

#[derive(Default)]
struct Partial {
    href: Option<String>,
    status: Option<u16>,
    props: Props,
    stat_props: Props,
    stat_status: Option<u16>,
}

fn merge(into: &mut Props, from: Props) {
    let Props {
        current_user_principal,
        calendar_home_set,
        addressbook_home_set,
        address_set,
        displayname,
        resource_types,
        components,
        privileges,
        color,
        ctag,
        etag,
        calendar_data,
        address_data,
        sync_token,
        max_resource_size,
        me_card,
    } = from;
    into.current_user_principal = into
        .current_user_principal
        .take()
        .or(current_user_principal);
    into.calendar_home_set = into.calendar_home_set.take().or(calendar_home_set);
    into.addressbook_home_set = into.addressbook_home_set.take().or(addressbook_home_set);
    into.address_set.extend(address_set);
    into.displayname = into.displayname.take().or(displayname);
    into.resource_types.extend(resource_types);
    into.components.extend(components);
    into.privileges.extend(privileges);
    into.color = into.color.take().or(color);
    into.ctag = into.ctag.take().or(ctag);
    into.etag = into.etag.take().or(etag);
    into.calendar_data = into.calendar_data.take().or(calendar_data);
    into.address_data = into.address_data.take().or(address_data);
    into.sync_token = into.sync_token.take().or(sync_token);
    into.max_resource_size = into.max_resource_size.take().or(max_resource_size);
    into.me_card = into.me_card.take().or(me_card);
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn bad(e: impl std::fmt::Display) -> DavError {
    DavError::Upstream(format!("unreadable multistatus response: {e}"))
}

fn namespace(r: &ResolveResult) -> String {
    match r {
        ResolveResult::Bound(ns) => ns.as_ref().to_string(),
        _ => String::new(),
    }
}

fn comp_name(e: &BytesStart) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == "name")
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Parses a `207 Multi-Status` body into one entry per `response`.
pub fn parse_multistatus(xml: &str) -> Result<Vec<DavResponse>> {
    Ok(parse_sync_report(xml)?.responses)
}

/// Parses a `207 Multi-Status` body, keeping the top-level `sync-token` of a sync-collection
/// report.
pub fn parse_sync_report(xml: &str) -> Result<Multistatus> {
    let mut reader = NsReader::from_str(xml);
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut text = String::new();
    let mut out = Multistatus::default();
    let mut cur: Option<Partial> = None;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(bad)?;
        let ns = namespace(&ns);
        match event {
            Event::Start(e) => {
                let local = e.local_name().as_ref().to_string();
                on_open(&stack, &ns, &local, &e, &mut cur);
                stack.push((ns, local));
                text.clear();
            }
            Event::Empty(e) => {
                let local = e.local_name().as_ref().to_string();
                on_open(&stack, &ns, &local, &e, &mut cur);
                stack.push((ns, local));
                text.clear();
                on_close(&mut stack, &mut text, &mut cur, &mut out);
            }
            Event::End(_) => on_close(&mut stack, &mut text, &mut cur, &mut out),
            Event::Text(t) => text.push_str(&t.into_inner()),
            Event::CData(t) => text.push_str(&t.into_inner()),
            Event::GeneralRef(r) => {
                let entity = format!("&{};", r.as_ref());
                let resolved = quick_xml::escape::unescape(&entity).map_err(bad)?;
                text.push_str(&resolved);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn parent_is(stack: &[(String, String)], depth_from_top: usize, ns: &str, local: &str) -> bool {
    stack
        .len()
        .checked_sub(depth_from_top + 1)
        .and_then(|i| stack.get(i))
        .is_some_and(|(n, l)| n == ns && l == local)
}

fn on_open(
    stack: &[(String, String)],
    ns: &str,
    local: &str,
    e: &BytesStart,
    cur: &mut Option<Partial>,
) {
    if ns == DAV && local == "response" {
        *cur = Some(Partial::default());
        return;
    }
    let Some(p) = cur.as_mut() else { return };
    if ns == DAV && local == "propstat" {
        p.stat_props = Props::default();
        p.stat_status = None;
        return;
    }
    if parent_is(stack, 0, DAV, "resourcetype") {
        p.stat_props.resource_types.push(clark(ns, local));
        return;
    }
    if ns == CALDAV
        && local == "comp"
        && parent_is(stack, 0, CALDAV, "supported-calendar-component-set")
    {
        if let Some(name) = comp_name(e) {
            p.stat_props.components.push(name.to_ascii_uppercase());
        }
        return;
    }
    if parent_is(stack, 0, DAV, "privilege") {
        p.stat_props.privileges.push(local.to_string());
    }
}

fn on_close(
    stack: &mut Vec<(String, String)>,
    text: &mut String,
    cur: &mut Option<Partial>,
    out: &mut Multistatus,
) {
    let Some((ns, local)) = stack.pop() else {
        return;
    };
    let value = std::mem::take(text);
    let trimmed = value.trim().to_string();
    if ns == DAV && local == "response" {
        if let Some(p) = cur.take()
            && let Some(href) = p.href
        {
            out.responses.push(DavResponse {
                href,
                status: p.status,
                props: p.props,
            });
        }
        return;
    }
    if ns == DAV && local == "sync-token" && parent_is(stack, 0, DAV, "multistatus") {
        out.sync_token = non_empty(trimmed);
        return;
    }
    let Some(p) = cur.as_mut() else { return };
    let (ns, local) = (ns.as_str(), local.as_str());
    match (ns, local) {
        (DAV, "propstat") => {
            let stat = std::mem::take(&mut p.stat_props);
            if p.stat_status.is_some_and(|s| (200..300).contains(&s)) {
                merge(&mut p.props, stat);
            }
        }
        (DAV, "status") if parent_is(stack, 0, DAV, "propstat") => {
            p.stat_status = status_code(&trimmed);
        }
        (DAV, "status") if parent_is(stack, 0, DAV, "response") => {
            p.status = status_code(&trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, DAV, "response") => p.href = non_empty(trimmed),
        (DAV, "href") if parent_is(stack, 0, DAV, "current-user-principal") => {
            p.stat_props.current_user_principal = non_empty(trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, CALDAV, "calendar-home-set") => {
            p.stat_props.calendar_home_set = non_empty(trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, CARDDAV, "addressbook-home-set") => {
            p.stat_props.addressbook_home_set = non_empty(trimmed);
        }
        (DAV, "href") if parent_is(stack, 0, CALDAV, "calendar-user-address-set") => {
            if !trimmed.is_empty() {
                p.stat_props.address_set.push(trimmed);
            }
        }
        (DAV, "href") if parent_is(stack, 0, CALSERVER, "me-card") => {
            p.stat_props.me_card = non_empty(trimmed);
        }
        (DAV, "displayname") => p.stat_props.displayname = non_empty(trimmed),
        (DAV, "getetag") => p.stat_props.etag = non_empty(trimmed),
        (DAV, "sync-token") => p.stat_props.sync_token = non_empty(trimmed),
        (APPLE, "calendar-color") => p.stat_props.color = non_empty(trimmed),
        (CALSERVER, "getctag") => p.stat_props.ctag = non_empty(trimmed),
        (CALDAV, "calendar-data") => p.stat_props.calendar_data = non_empty(trimmed),
        (CARDDAV, "address-data") => p.stat_props.address_data = non_empty(trimmed),
        (CARDDAV, "max-resource-size") => p.stat_props.max_resource_size = trimmed.parse().ok(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PROPFIND Depth 0 on a principal for the address book home, shaped like iCloud's answer.
    const PRINCIPAL_CARD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
	<response>
		<href>/1234567890/principal/</href>
		<propstat>
			<prop>
				<addressbook-home-set xmlns="urn:ietf:params:xml:ns:carddav">
					<href xmlns="DAV:">https://p99-contacts.icloud.com:443/1234567890/carddavhome/</href>
				</addressbook-home-set>
			</prop>
			<status>HTTP/1.1 200 OK</status>
		</propstat>
		<propstat>
			<prop>
				<principal-address xmlns="urn:ietf:params:xml:ns:carddav"/>
			</prop>
			<status>HTTP/1.1 404 Not Found</status>
		</propstat>
	</response>
</multistatus>"#;

    /// PROPFIND Depth 1 on the address book home.
    const BOOKS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
	<response>
		<href>/1234567890/carddavhome/</href>
		<propstat>
			<prop>
				<me-card xmlns="http://calendarserver.org/ns/">
					<href xmlns="DAV:">/1234567890/carddavhome/card/NGZkNzJkZGEtN2FkYS00NzAy.vcf</href>
				</me-card>
				<resourcetype><collection/></resourcetype>
				<sync-token>HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYI</sync-token>
			</prop>
			<status>HTTP/1.1 200 OK</status>
		</propstat>
	</response>
	<response>
		<href>/1234567890/carddavhome/card/</href>
		<propstat>
			<prop>
				<resourcetype><collection/><addressbook xmlns="urn:ietf:params:xml:ns:carddav"/></resourcetype>
				<displayname>card</displayname>
				<sync-token>HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYJ</sync-token>
				<max-resource-size xmlns="urn:ietf:params:xml:ns:carddav">563200</max-resource-size>
			</prop>
			<status>HTTP/1.1 200 OK</status>
		</propstat>
		<propstat>
			<prop><me-card xmlns="http://calendarserver.org/ns/"/></prop>
			<status>HTTP/1.1 404 Not Found</status>
		</propstat>
	</response>
</multistatus>"#;

    /// sync-collection answer: one changed card, one deleted card, and the new token.
    const SYNC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
	<response>
		<href>/1234567890/carddavhome/card/00000000-0000-4000-8000-000000000001.vcf</href>
		<propstat>
			<prop><getetag>"mvcq4plv"</getetag></prop>
			<status>HTTP/1.1 200 OK</status>
		</propstat>
	</response>
	<response>
		<href>/1234567890/carddavhome/card/gone.vcf</href>
		<status>HTTP/1.1 404 Not Found</status>
	</response>
	<sync-token>HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYK</sync-token>
</multistatus>"#;

    /// addressbook-multiget answer: an entity-escaped card, a CDATA card and an unknown href.
    const MULTIGET: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<multistatus xmlns=\"DAV:\">\n\t<response>\n\t\t<href>/1234567890/carddavhome/card/a.vcf</href>\n\t\t<propstat>\n\t\t\t<prop>\n\t\t\t\t<getetag>\"e1\"</getetag>\n\t\t\t\t<address-data xmlns=\"urn:ietf:params:xml:ns:carddav\">BEGIN:VCARD&#13;\nVERSION:3.0&#13;\nFN:Fish &amp; Chips Ltd&#13;\nNOTE:a &lt;b&gt;&#13;\nEND:VCARD&#13;\n</address-data>\n\t\t\t</prop>\n\t\t\t<status>HTTP/1.1 200 OK</status>\n\t\t</propstat>\n\t</response>\n\t<response>\n\t\t<href>/1234567890/carddavhome/card/b.vcf</href>\n\t\t<propstat>\n\t\t\t<prop>\n\t\t\t\t<getetag>\"e2\"</getetag>\n\t\t\t\t<address-data xmlns=\"urn:ietf:params:xml:ns:carddav\"><![CDATA[BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Example User\r\nEND:VCARD\r\n]]></address-data>\n\t\t\t</prop>\n\t\t\t<status>HTTP/1.1 200 OK</status>\n\t\t</propstat>\n\t</response>\n\t<response>\n\t\t<href>/1234567890/carddavhome/card/does-not-exist.vcf</href>\n\t\t<status>HTTP/1.1 404 Not Found</status>\n\t</response>\n</multistatus>";

    fn well_formed(body: &str) {
        let mut r = NsReader::from_str(body);
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

    #[test]
    fn request_bodies_are_well_formed_and_namespaced() {
        for body in [
            propfind_principal(),
            sync_collection(None),
            sync_collection(Some("HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYK")),
            addressbook_multiget(&["/1234567890/carddavhome/card/a.vcf".to_string()]),
        ] {
            well_formed(&body);
        }
    }

    #[test]
    fn sync_collection_sends_the_token_escaped_or_an_empty_one() {
        let first = sync_collection(None);
        assert!(first.contains("<d:sync-token/>"), "{first}");
        assert!(first.contains("<d:sync-level>1</d:sync-level>"));
        assert!(first.contains("<d:prop><d:getetag/></d:prop>"));
        let next = sync_collection(Some("https://example.com/sync/1?a=1&b=<2>"));
        assert!(
            next.contains(
                "<d:sync-token>https://example.com/sync/1?a=1&amp;b=&lt;2&gt;</d:sync-token>"
            ),
            "{next}"
        );
    }

    #[test]
    fn addressbook_multiget_lists_every_href_escaped() {
        let body = addressbook_multiget(&[
            "/1234567890/carddavhome/card/a.vcf".to_string(),
            "/1234567890/carddavhome/card/b&c.vcf".to_string(),
        ]);
        assert!(body.contains("<card:address-data/>"), "{body}");
        assert!(body.contains("<d:getetag/>"));
        assert!(body.contains("<d:href>/1234567890/carddavhome/card/a.vcf</d:href>"));
        assert!(body.contains("<d:href>/1234567890/carddavhome/card/b&amp;c.vcf</d:href>"));
        well_formed(&body);
    }

    #[test]
    fn address_book_home_set_is_read_and_404_propstats_ignored() {
        let r = parse_multistatus(PRINCIPAL_CARD).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].props.addressbook_home_set.as_deref(),
            Some("https://p99-contacts.icloud.com:443/1234567890/carddavhome/")
        );
        assert_eq!(r[0].props.calendar_home_set, None);
    }

    #[test]
    fn me_card_book_type_size_and_property_sync_tokens_are_read() {
        let r = parse_multistatus(BOOKS).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[0].props.me_card.as_deref(),
            Some("/1234567890/carddavhome/card/NGZkNzJkZGEtN2FkYS00NzAy.vcf")
        );
        assert_eq!(
            r[0].props.sync_token.as_deref(),
            Some("HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYI")
        );
        assert!(!r[0].has_type(CARDDAV, "addressbook"));
        let book = &r[1];
        assert!(book.has_type(CARDDAV, "addressbook"));
        assert_eq!(book.props.displayname.as_deref(), Some("card"));
        assert_eq!(book.props.max_resource_size, Some(563200));
        assert_eq!(
            book.props.sync_token.as_deref(),
            Some("HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYJ")
        );
        assert_eq!(
            book.props.me_card, None,
            "the 404 propstat must not leak in"
        );
    }

    #[test]
    fn sync_report_reads_changes_deletions_and_the_new_token() {
        let m = parse_sync_report(SYNC).unwrap();
        assert_eq!(
            m.sync_token.as_deref(),
            Some("HwoQEgwAAEAAl9tUKgAAAAAYARgAIhYK")
        );
        assert_eq!(m.responses.len(), 2);
        assert_eq!(
            m.responses[0].href,
            "/1234567890/carddavhome/card/00000000-0000-4000-8000-000000000001.vcf"
        );
        assert_eq!(m.responses[0].status, None);
        assert_eq!(m.responses[0].props.etag.as_deref(), Some("\"mvcq4plv\""));
        assert_eq!(m.responses[1].href, "/1234567890/carddavhome/card/gone.vcf");
        assert_eq!(m.responses[1].status, Some(404));
        assert_eq!(m.responses[1].props, Props::default());
        assert_eq!(parse_multistatus(SYNC).unwrap(), m.responses);
    }

    #[test]
    fn multiget_reads_entity_escaped_and_cdata_vcards() {
        let r = parse_multistatus(MULTIGET).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].props.etag.as_deref(), Some("\"e1\""));
        assert_eq!(
            r[0].props.address_data.as_deref(),
            Some("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Fish & Chips Ltd\r\nNOTE:a <b>\r\nEND:VCARD")
        );
        assert_eq!(
            r[1].props.address_data.as_deref(),
            Some("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Example User\r\nEND:VCARD")
        );
        assert_eq!(r[2].status, Some(404));
        assert_eq!(r[2].props.address_data, None);
    }

    #[test]
    fn a_non_numeric_resource_size_is_ignored() {
        let xml = r#"<multistatus xmlns="DAV:"><response><href>/c/</href><propstat><prop><max-resource-size xmlns="urn:ietf:params:xml:ns:carddav">big</max-resource-size></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        assert_eq!(
            parse_multistatus(xml).unwrap()[0].props.max_resource_size,
            None
        );
    }

    #[test]
    fn garbage_is_an_upstream_error() {
        let e = parse_multistatus("<multistatus xmlns=\"DAV:\"><response></nope>").unwrap_err();
        assert!(matches!(e, DavError::Upstream(_)), "{e:?}");
        assert_eq!(e.kind(), "upstream");
    }

    #[test]
    fn empty_multistatus_is_empty() {
        let m = parse_sync_report(r#"<?xml version="1.0"?><multistatus xmlns="DAV:"/>"#).unwrap();
        assert!(m.responses.is_empty());
        assert_eq!(m.sync_token, None);
    }
}
