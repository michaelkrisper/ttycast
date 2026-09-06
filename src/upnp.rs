//! Minimal UPnP/DLNA control point: find a MediaRenderer, hand it a URL.
//!
//! Nothing but an address travels to the TV - it opens the stream itself,
//! which is why the sender stays cheap no matter how large the picture gets.
//!
//! Written against the UPnP AVTransport:1 specification. Many TVs only publish
//! their renderer while screen/content sharing is switched on, so an empty
//! discovery result is usually a setting on the TV, not a bug here.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

pub const SSDP_ADDR: &str = "239.255.255.250:1900";
pub const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
pub const AVTRANSPORT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const USER_AGENT: &str = "ttycast/0.2 UPnP/1.0";

/// Live source: no seeking, no byte ranges, sender-paced.
pub const DLNA_FEATURES_LIVE: &str =
    "DLNA.ORG_OP=00;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=8D500000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renderer {
    pub name: String,
    pub location: String,
    pub control_url: String,
    pub address: String,
}

impl Renderer {
    #[must_use]
    pub fn host(&self) -> String {
        host_of(&self.location).unwrap_or_else(|| self.address.clone())
    }
}

/// Read the text inside `<name>` or `<ns:name>`, ignoring attributes.
#[must_use]
pub fn tag(xml: &str, name: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(open) = xml[cursor..].find('<') {
        let start = cursor + open + 1;
        let rest = &xml[start..];
        let end = rest.find('>')? + start;
        let inner = &xml[start..end];
        // Take the element name off first, then drop its namespace prefix:
        // the other order walks into attribute values like xmlns:dlna="urn:a:b".
        let element = inner.split_whitespace().next().unwrap_or(inner);
        let bare = element.rsplit(':').next().unwrap_or(element);
        if bare == name && !inner.starts_with('/') {
            let close = xml[end + 1..].find("</")? + end + 1;
            return Some(xml[end + 1..close].trim().to_string());
        }
        cursor = end;
    }
    None
}

/// Case-insensitive header lookup in an SSDP or HTTP message.
#[must_use]
pub fn header(message: &str, name: &str) -> String {
    let wanted = name.to_ascii_lowercase();
    for line in message.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.trim().to_ascii_lowercase() == wanted
        {
            return value.trim().to_string();
        }
    }
    String::new()
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?']).next()?;
    Some(
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
            .to_string(),
    )
}

fn authority_of(url: &str) -> Option<(String, u16, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" {
        return None;
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority
        .rsplit_once(':')
        .and_then(|(h, p)| p.parse().ok().map(|p: u16| (h.to_string(), p)))
        .unwrap_or((authority.to_string(), 80));
    Some((host, port, format!("/{path}")))
}

/// Resolve a possibly relative `controlURL` against the description's location.
#[must_use]
pub fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return path.to_string();
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    if path.starts_with('/') {
        format!("{scheme}://{authority}{path}")
    } else {
        let dir = base.rsplit_once('/').map_or(base, |(head, _)| head);
        format!("{dir}/{path}")
    }
}

/// M-SEARCH the LAN; map responder address to its device description URL.
#[must_use]
pub fn discover_raw(timeout: Duration) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return found;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
    let request = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\n\
         ST: {RENDERER}\r\nUSER-AGENT: {USER_AGENT}\r\n\r\n"
    );
    let Ok(target) = SSDP_ADDR.to_socket_addrs().map(|mut a| a.next()) else {
        return found;
    };
    let Some(target) = target else { return found };
    if socket.send_to(request.as_bytes(), target).is_err() {
        return found;
    }

    let deadline = Instant::now() + timeout;
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        if let Ok((read, from)) = socket.recv_from(&mut buffer) {
            let message = String::from_utf8_lossy(&buffer[..read]);
            let location = header(&message, "location");
            if !location.is_empty() {
                found.entry(from.ip().to_string()).or_insert(location);
            }
        }
    }
    found
}

/// A very small HTTP/1.1 client: enough for a device description and a SOAP call.
pub fn http(
    url: &str,
    method: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<String, String> {
    let (host, port, path) = authority_of(url).ok_or_else(|| format!("not an http url: {url}"))?;
    let address = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {host}"))?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|e| format!("cannot reach {host}:{port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|e| e.to_string())?;

    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: {USER_AGENT}\r\n\
         Connection: close\r\n"
    );
    for (key, value) in headers {
        let _ = write!(request, "{key}: {value}\r\n");
    }
    if let Some(body) = body {
        let _ = write!(request, "Content-Length: {}\r\n", body.len());
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("cannot send to {host}: {e}"))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("cannot read from {host}: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    if status.starts_with('2') {
        Ok(body.to_string())
    } else {
        Err(format!("{url} answered {status}"))
    }
}

/// Pull the AVTransport `controlURL` out of a device description.
#[must_use]
pub fn control_url(description: &str, base: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(open) = description[cursor..].find("<service") {
        let start = cursor + open;
        let end = description[start..].find("</service>").map(|e| start + e)?;
        let block = &description[start..end];
        if block.contains(AVTRANSPORT)
            && let Some(path) = tag(block, "controlURL")
        {
            return Some(join_url(base, &path));
        }
        cursor = end + 1;
    }
    None
}

pub fn describe(address: &str, location: &str) -> Option<Renderer> {
    let xml = http(location, "GET", &[], None).ok()?;
    let control = control_url(&xml, location)?;
    Some(Renderer {
        name: tag(&xml, "friendlyName").unwrap_or_else(|| address.to_string()),
        location: location.to_string(),
        control_url: control,
        address: address.to_string(),
    })
}

/// Every MediaRenderer on the LAN that exposes AVTransport.
#[must_use]
pub fn discover(timeout: Duration) -> Vec<Renderer> {
    let mut renderers: Vec<Renderer> = discover_raw(timeout)
        .iter()
        .filter_map(|(address, location)| describe(address, location))
        .collect();
    renderers.sort_by_key(|r| r.name.to_lowercase());
    renderers
}

#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[must_use]
pub fn soap_body(action: &str, arguments: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{AVTRANSPORT}\"><InstanceID>0</InstanceID>{arguments}\
         </u:{action}></s:Body></s:Envelope>"
    )
}

pub fn soap(control: &str, action: &str, arguments: &str) -> Result<String, String> {
    let body = soap_body(action, arguments);
    let soap_action = format!("\"{AVTRANSPORT}#{action}\"");
    http(
        control,
        "POST",
        &[
            ("Content-Type", "text/xml; charset=\"utf-8\""),
            ("SOAPAction", &soap_action),
        ],
        Some(&body),
    )
}

/// DIDL-Lite metadata for a live video item.
///
/// A renderer given no metadata often refuses the URI outright, and one given
/// a `duration` will try to draw a position bar for a live stream, so neither
/// size nor duration is declared here.
#[must_use]
pub fn didl(title: &str, url: &str, mime: &str) -> String {
    let protocol = format!("http-get:*:{mime}:{DLNA_FEATURES_LIVE}");
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">\
         <item id=\"ttycast\" parentID=\"0\" restricted=\"1\">\
         <dc:title>{}</dc:title>\
         <upnp:class>object.item.videoItem</upnp:class>\
         <res protocolInfo=\"{}\">{}</res>\
         </item></DIDL-Lite>",
        escape(title),
        escape(&protocol),
        escape(url)
    )
}

/// `SetAVTransportURI` then `Play` - the whole handover.
pub fn play(renderer: &Renderer, url: &str, title: &str, mime: &str) -> Result<(), String> {
    let metadata = didl(title, url, mime);
    soap(
        &renderer.control_url,
        "SetAVTransportURI",
        &format!(
            "<CurrentURI>{}</CurrentURI><CurrentURIMetaData>{}</CurrentURIMetaData>",
            escape(url),
            escape(&metadata)
        ),
    )?;
    soap(&renderer.control_url, "Play", "<Speed>1</Speed>")?;
    Ok(())
}

pub fn stop(renderer: &Renderer) -> Result<(), String> {
    soap(&renderer.control_url, "Stop", "").map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
<friendlyName>Smart TV</friendlyName>
<dlna:X_DLNADOC xmlns:dlna="urn:schemas-dlna-org:device-1-0">DMR-1.50</dlna:X_DLNADOC>
<serviceList>
  <service>
    <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
    <controlURL>/upnp/control/cm</controlURL>
  </service>
  <service>
    <serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>
    <controlURL>/upnp/control/avt</controlURL>
  </service>
</serviceList></device></root>"#;

    const RESPONSE: &str = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\n\
LOCATION: http://192.168.1.217:49152/description.xml\r\n\
ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";

    #[test]
    fn header_lookup_is_case_insensitive() {
        assert_eq!(
            header(RESPONSE, "location"),
            "http://192.168.1.217:49152/description.xml"
        );
        assert!(header(RESPONSE, "LOCATION").ends_with("description.xml"));
        assert_eq!(header(RESPONSE, "missing"), "");
    }

    #[test]
    fn tag_reads_namespaced_and_plain_elements() {
        assert_eq!(
            tag(DESCRIPTION, "friendlyName").as_deref(),
            Some("Smart TV")
        );
        assert_eq!(tag(DESCRIPTION, "X_DLNADOC").as_deref(), Some("DMR-1.50"));
        assert_eq!(tag(DESCRIPTION, "nope"), None);
    }

    #[test]
    fn control_url_picks_avtransport_and_resolves_relative_paths() {
        let url = control_url(DESCRIPTION, "http://192.168.1.217:49152/description.xml");
        assert_eq!(
            url.as_deref(),
            Some("http://192.168.1.217:49152/upnp/control/avt")
        );
    }

    #[test]
    fn control_url_returns_none_without_avtransport() {
        let stripped = DESCRIPTION.replace("AVTransport:1", "RenderingControl:1");
        assert_eq!(control_url(&stripped, "http://x/"), None);
    }

    #[test]
    fn join_url_handles_all_three_forms() {
        assert_eq!(join_url("http://h:1/a/b.xml", "/c"), "http://h:1/c");
        assert_eq!(join_url("http://h:1/a/b.xml", "c"), "http://h:1/a/c");
        assert_eq!(join_url("http://h:1/a", "http://other/x"), "http://other/x");
    }

    #[test]
    fn soap_body_is_well_formed() {
        let body = soap_body("Play", "<Speed>1</Speed>");
        assert!(body.starts_with("<?xml version=\"1.0\""));
        assert!(body.contains(&format!("<u:Play xmlns:u=\"{AVTRANSPORT}\">")));
        assert!(body.contains("<InstanceID>0</InstanceID><Speed>1</Speed>"));
        assert!(body.ends_with("</u:Play></s:Body></s:Envelope>"));
    }

    #[test]
    fn didl_declares_a_live_resource() {
        let metadata = didl("ttycast", "http://10.0.0.5:8009/live.ts", "video/mpeg");
        assert!(metadata.contains("object.item.videoItem"));
        assert!(metadata.contains("DLNA.ORG_OP=00"));
        // A live stream must not claim a duration or a renderer draws a seek bar.
        assert!(!metadata.contains("duration"));
        assert!(!metadata.contains("size="));
    }

    #[test]
    fn didl_escapes_the_url_and_title() {
        let metadata = didl("a & b", "http://h/x?a=1&b=2", "video/mpeg");
        assert!(metadata.contains("a &amp; b"));
        assert!(metadata.contains("a=1&amp;b=2"));
        assert!(!metadata.contains("&b=2"));
    }

    #[test]
    fn renderer_host_comes_from_the_location() {
        let renderer = Renderer {
            name: "TV".into(),
            location: "http://192.168.1.217:49152/d.xml".into(),
            control_url: "http://x/c".into(),
            address: "10.0.0.1".into(),
        };
        assert_eq!(renderer.host(), "192.168.1.217");
    }

    #[test]
    fn http_rejects_a_url_it_cannot_speak() {
        assert!(http("ftp://example.com/x", "GET", &[], None).is_err());
    }
}
