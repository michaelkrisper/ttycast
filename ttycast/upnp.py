"""Minimal UPnP/DLNA control point: find a MediaRenderer, hand it a URL.

Nothing but an address travels to the TV - it opens the stream itself, which is
why the sender stays cheap no matter how large the picture gets.

Written against the UPnP AVTransport:1 specification. Note that many TVs only
publish their renderer while screen/content sharing is switched on, so an empty
discovery result is usually a setting on the TV, not a bug here.
"""

from __future__ import annotations

import re
import socket
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from xml.sax.saxutils import escape

SSDP_ADDR = "239.255.255.250"
SSDP_PORT = 1900
RENDERER = "urn:schemas-upnp-org:device:MediaRenderer:1"
AVTRANSPORT = "urn:schemas-upnp-org:service:AVTransport:1"
USER_AGENT = "ttycast/0.1 UPnP/1.0"

#: Live source: no seeking, no byte ranges, sender-paced.
DLNA_FEATURES_LIVE = "DLNA.ORG_OP=00;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=8D500000000000000000000000000000"

_TAG = re.compile(r"<(?:\w+:)?{name}[^>]*>(.*?)</(?:\w+:)?{name}>", re.S)


def tag(xml: str, name: str) -> str | None:
    m = re.search(_TAG.pattern.format(name=re.escape(name)), xml, re.S)
    return m.group(1).strip() if m else None


def header(message: str, name: str) -> str:
    prefix = name.lower() + ":"
    for line in message.splitlines():
        if line.lower().startswith(prefix):
            return line.split(":", 1)[1].strip()
    return ""


@dataclass(frozen=True, slots=True)
class Renderer:
    name: str
    location: str
    control_url: str
    address: str

    @property
    def host(self) -> str:
        return urllib.parse.urlparse(self.location).hostname or self.address


def discover_raw(timeout: float = 3.0, st: str = RENDERER) -> dict[str, str]:
    """M-SEARCH the LAN; map responder address to its device description URL."""
    request = (
        "M-SEARCH * HTTP/1.1\r\n"
        f"HOST: {SSDP_ADDR}:{SSDP_PORT}\r\n"
        'MAN: "ssdp:discover"\r\n'
        "MX: 2\r\n"
        f"ST: {st}\r\n"
        f"USER-AGENT: {USER_AGENT}\r\n"
        "\r\n"
    ).encode()

    found: dict[str, str] = {}
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 2)
    sock.settimeout(timeout)
    try:
        sock.sendto(request, (SSDP_ADDR, SSDP_PORT))
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            sock.settimeout(max(0.1, end - time.monotonic()))
            try:
                data, addr = sock.recvfrom(65507)
            except (TimeoutError, OSError):
                break
            location = header(data.decode("utf-8", "replace"), "location")
            if location:
                found.setdefault(addr[0], location)
    finally:
        sock.close()
    return found


def fetch(url: str, timeout: float = 5.0) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})  # noqa: S310
    with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
        return resp.read().decode("utf-8", "replace")


def control_url(description: str, base: str) -> str | None:
    """Pull the AVTransport ``controlURL`` out of a device description."""
    for block in re.findall(r"<service>(.*?)</service>", description, re.S):
        if AVTRANSPORT in block:
            path = tag(block, "controlURL")
            if path:
                return urllib.parse.urljoin(base, path)
    return None


def describe(address: str, location: str) -> Renderer | None:
    try:
        xml = fetch(location)
    except (urllib.error.URLError, OSError, TimeoutError):
        return None
    ctl = control_url(xml, location)
    if ctl is None:
        return None
    return Renderer(
        name=tag(xml, "friendlyName") or address,
        location=location,
        control_url=ctl,
        address=address,
    )


def discover(timeout: float = 3.0) -> list[Renderer]:
    """Every MediaRenderer on the LAN that exposes AVTransport."""
    out = []
    for address, location in discover_raw(timeout).items():
        renderer = describe(address, location)
        if renderer is not None:
            out.append(renderer)
    return sorted(out, key=lambda r: r.name.lower())


def soap_body(action: str, arguments: str) -> bytes:
    return (
        '<?xml version="1.0" encoding="utf-8"?>'
        '<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" '
        's:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body>'
        f'<u:{action} xmlns:u="{AVTRANSPORT}"><InstanceID>0</InstanceID>{arguments}'
        f"</u:{action}></s:Body></s:Envelope>"
    ).encode()


def soap(control: str, action: str, arguments: str = "", timeout: float = 8.0) -> str:
    req = urllib.request.Request(  # noqa: S310
        control,
        data=soap_body(action, arguments),
        headers={
            "Content-Type": 'text/xml; charset="utf-8"',
            "SOAPAction": f'"{AVTRANSPORT}#{action}"',
            "User-Agent": USER_AGENT,
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
        return resp.read().decode("utf-8", "replace")


def didl(title: str, url: str, mime: str = "video/mpeg") -> str:
    """DIDL-Lite metadata for a live video item.

    A renderer that gets no metadata often refuses the URI outright, and one
    that gets a ``duration`` will try to draw a position bar for a live stream,
    so neither ``size`` nor ``duration`` is declared here.
    """
    protocol = f"http-get:*:{mime}:{DLNA_FEATURES_LIVE}"
    return (
        '<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/" '
        'xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">'
        '<item id="ttycast" parentID="0" restricted="1">'
        f"<dc:title>{escape(title)}</dc:title>"
        "<upnp:class>object.item.videoItem</upnp:class>"
        f'<res protocolInfo="{escape(protocol)}">{escape(url)}</res>'
        "</item></DIDL-Lite>"
    )


def play(renderer: Renderer, url: str, title: str = "ttycast", mime: str = "video/mpeg") -> None:
    """``SetAVTransportURI`` then ``Play`` - the whole handover."""
    metadata = didl(title, url, mime)
    soap(
        renderer.control_url,
        "SetAVTransportURI",
        f"<CurrentURI>{escape(url)}</CurrentURI>"
        f"<CurrentURIMetaData>{escape(metadata)}</CurrentURIMetaData>",
    )
    soap(renderer.control_url, "Play", "<Speed>1</Speed>")


def stop(renderer: Renderer) -> None:
    soap(renderer.control_url, "Stop")
