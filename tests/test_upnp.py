import pytest

from ttycast import upnp

DESCRIPTION = """<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
<friendlyName>Smart TV</friendlyName>
<serviceList>
  <service>
    <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
    <controlURL>/upnp/control/cm</controlURL>
  </service>
  <service>
    <serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>
    <controlURL>/upnp/control/avt</controlURL>
  </service>
</serviceList></device></root>"""

RESPONSE = (
    "HTTP/1.1 200 OK\r\n"
    "CACHE-CONTROL: max-age=1800\r\n"
    "LOCATION: http://192.168.1.217:49152/description.xml\r\n"
    "ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n"
)


def test_header_lookup_is_case_insensitive():
    assert upnp.header(RESPONSE, "location") == "http://192.168.1.217:49152/description.xml"
    assert upnp.header(RESPONSE, "LOCATION").endswith("description.xml")
    assert upnp.header(RESPONSE, "missing") == ""


def test_tag_reads_namespaced_and_plain_elements():
    assert upnp.tag(DESCRIPTION, "friendlyName") == "Smart TV"
    assert upnp.tag("<dlna:X_DLNADOC>DMR-1.50</dlna:X_DLNADOC>", "X_DLNADOC") == "DMR-1.50"
    assert upnp.tag(DESCRIPTION, "nope") is None


def test_control_url_picks_avtransport_and_resolves_relative_paths():
    url = upnp.control_url(DESCRIPTION, "http://192.168.1.217:49152/description.xml")
    assert url == "http://192.168.1.217:49152/upnp/control/avt"


def test_control_url_returns_none_without_avtransport():
    stripped = DESCRIPTION.replace("AVTransport:1", "RenderingControl:1")
    assert upnp.control_url(stripped, "http://x/") is None


def test_soap_body_is_well_formed():
    body = upnp.soap_body("Play", "<Speed>1</Speed>").decode()
    assert body.startswith('<?xml version="1.0"')
    assert '<u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">' in body
    assert "<InstanceID>0</InstanceID><Speed>1</Speed>" in body
    assert body.endswith("</u:Play></s:Body></s:Envelope>")


def test_didl_declares_a_live_resource():
    metadata = upnp.didl("ttycast", "http://10.0.0.5:8009/live.ts")
    assert "object.item.videoItem" in metadata
    assert "DLNA.ORG_OP=00" in metadata
    # A live stream must not claim a duration or a renderer draws a seek bar.
    assert "duration" not in metadata
    assert "size=" not in metadata


def test_didl_escapes_the_url_and_title():
    metadata = upnp.didl("a & b", "http://h/x?a=1&b=2")
    assert "a &amp; b" in metadata
    assert "a=1&amp;b=2" in metadata
    assert "&b=2" not in metadata


def test_renderer_host_comes_from_the_location():
    renderer = upnp.Renderer("TV", "http://192.168.1.217:49152/d.xml", "http://x/c", "10.0.0.1")
    assert renderer.host == "192.168.1.217"


@pytest.mark.parametrize("action", ["SetAVTransportURI", "Stop"])
def test_soap_body_covers_every_action_we_send(action):
    assert f"<u:{action}".encode() in upnp.soap_body(action, "")
