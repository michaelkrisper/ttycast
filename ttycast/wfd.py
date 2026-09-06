"""The Wi-Fi Display (Miracast) source side of the RTSP handshake.

Miracast is three layers stacked: Wi-Fi Direct carries the link, RTSP negotiates
the session (the M1..M7 exchange below), and the picture travels as H.264 inside
MPEG-TS over RTP/UDP. Only the middle layer lives here, as pure message
building and parsing, so it can be tested without a radio.

The link layer is *not* implemented in Python - see :mod:`ttycast.backends.miracast`,
which drives wpa_supplicant or MiracleCast for that part.

Reference: Wi-Fi Alliance "Wi-Fi Display Technical Specification" v2.x, section
6.4 (Capability Negotiation) and RFC 2326 (RTSP 1.0).
"""

from __future__ import annotations

from dataclasses import dataclass, field

RTSP_VERSION = "RTSP/1.0"
USER_AGENT = "ttycast/0.1"

#: The CEA/VESA/HH resolution bitmaps a source advertises in wfd_video_formats.
#: ttycast only ever offers progressive modes that every sink must support.
CEA_640x480p60 = 1 << 0
CEA_720x480p60 = 1 << 1
CEA_1280x720p30 = 1 << 5
CEA_1280x720p60 = 1 << 6
CEA_1920x1080p30 = 1 << 8

#: (bitmap, width, height, fps) for the modes ttycast is willing to send.
SUPPORTED_MODES: tuple[tuple[int, int, int, int], ...] = (
    (CEA_1920x1080p30, 1920, 1080, 30),
    (CEA_1280x720p60, 1280, 720, 60),
    (CEA_1280x720p30, 1280, 720, 30),
    (CEA_640x480p60, 640, 480, 60),
)


@dataclass(slots=True)
class Message:
    """An RTSP request or response, either direction."""

    #: ``("OPTIONS", "rtsp://...")`` for a request, ``None`` for a response.
    request: tuple[str, str] | None = None
    #: ``(code, reason)`` for a response, ``None`` for a request.
    status: tuple[int, str] | None = None
    headers: dict[str, str] = field(default_factory=dict)
    body: str = ""

    @property
    def cseq(self) -> int | None:
        raw = self.headers.get("cseq")
        return int(raw) if raw and raw.isdigit() else None

    def encode(self) -> bytes:
        headers = dict(self.headers)
        if self.body:
            headers.setdefault("content-type", "text/parameters")
            headers["content-length"] = str(len(self.body.encode()))
        if self.request is not None:
            method, url = self.request
            start = f"{method} {url} {RTSP_VERSION}"
        else:
            assert self.status is not None
            code, reason = self.status
            start = f"{RTSP_VERSION} {code} {reason}"
        lines = [start]
        lines += [f"{_canonical(k)}: {v}" for k, v in headers.items()]
        return ("\r\n".join(lines) + "\r\n\r\n" + self.body).encode()


def _canonical(name: str) -> str:
    """RTSP headers are case-insensitive, but sinks in the wild are picky."""
    special = {"cseq": "CSeq", "wfd_content_protection": "wfd_content_protection"}
    if name.startswith("wfd_"):
        return name
    return special.get(name, "-".join(part.capitalize() for part in name.split("-")))


def parse(raw: str) -> Message:
    """Parse one complete RTSP message (headers must already be terminated)."""
    head, _, body = raw.partition("\r\n\r\n")
    lines = head.split("\r\n")
    start = lines[0].split(" ", 2)
    message = Message(body=body)
    if start[0].startswith("RTSP/"):
        message.status = (int(start[1]), start[2] if len(start) > 2 else "")
    else:
        message.request = (start[0], start[1] if len(start) > 1 else "*")
    for line in lines[1:]:
        if ":" in line:
            key, value = line.split(":", 1)
            message.headers[key.strip().lower()] = value.strip()
    return message


def parse_parameters(body: str) -> dict[str, str]:
    """``wfd_audio_codecs: LPCM 00000002 00`` style bodies."""
    out: dict[str, str] = {}
    for line in body.splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            out[key.strip()] = value.strip()
        elif line.strip():
            out[line.strip()] = ""
    return out


def format_parameters(params: dict[str, str]) -> str:
    return "".join(f"{k}: {v}\r\n" if v else f"{k}\r\n" for k, v in params.items())


def response(cseq: int, code: int = 200, reason: str = "OK", **headers: str) -> Message:
    base = {"cseq": str(cseq), **headers}
    return Message(status=(code, reason), headers=base)


def request(method: str, url: str, cseq: int, body: str = "", **headers: str) -> Message:
    base = {"cseq": str(cseq), "user-agent": USER_AGENT, **headers}
    return Message(request=(method, url), headers=base, body=body)


def options_response(cseq: int) -> Message:
    """M1: the sink asks what we support."""
    return response(
        cseq,
        public="org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER",
    )


def options_request(url: str, cseq: int) -> Message:
    """M2: we ask the same of the sink."""
    return request("OPTIONS", url, cseq, require="org.wfa.wfd1.0")


def get_parameter_request(url: str, cseq: int, keys: list[str] | None = None) -> Message:
    """M3: ask the sink which formats and ports it can take."""
    keys = keys or [
        "wfd_video_formats",
        "wfd_audio_codecs",
        "wfd_client_rtp_ports",
        "wfd_content_protection",
    ]
    return request("GET_PARAMETER", url, cseq, body="".join(f"{k}\r\n" for k in keys))


def video_formats_value(bitmap: int, native_index: int = 0) -> str:
    """The ``wfd_video_formats`` value for a single offered CEA mode.

    Layout: native, preferred-display-mode, then one profile block of
    ``profile latency cea vesa hh min-slice slice-enc frame-skip max-hres max-vres``.
    """
    native = f"{native_index:02X}"
    return f"{native} 00 02 04 {bitmap:08X} 00000000 00000000 00 0000 0000 00 none none"


def choose_mode(sink_video_formats: str) -> tuple[int, int, int] | None:
    """Pick the best mode both sides support: ``(width, height, fps)``.

    ``sink_video_formats`` is the raw ``wfd_video_formats`` value the sink sent
    in its M3 reply. Only the CEA bitmap is considered - VESA and handheld modes
    exist but no TV needs them.
    """
    fields = sink_video_formats.split()
    if len(fields) < 5:
        return None
    try:
        cea = int(fields[4], 16)
    except ValueError:
        return None
    for bitmap, width, height, fps in SUPPORTED_MODES:
        if cea & bitmap:
            return width, height, fps
    return None


def set_parameter_body(
    rtp_port: int,
    bitmap: int,
    presentation_url: str = "rtsp://localhost/wfd1.0/streamid=0",
    client_host: str = "",
) -> str:
    """M4: tell the sink the format and where the RTP will come from."""
    params = {
        "wfd_video_formats": video_formats_value(bitmap),
        "wfd_audio_codecs": "none",
        "wfd_presentation_URL": f"{presentation_url} none",
        "wfd_client_rtp_ports": f"RTP/AVP/UDP;unicast {rtp_port} 0 mode=play",
    }
    if client_host:
        params["wfd_uibc_capability"] = "none"
    return format_parameters(params)


def trigger_body(method: str) -> str:
    """M5: ``SETUP``, ``PLAY``, ``PAUSE`` or ``TEARDOWN``."""
    return format_parameters({"wfd_trigger_method": method})


def parse_client_rtp_ports(value: str) -> int | None:
    """Pull the sink's RTP port out of ``wfd_client_rtp_ports``."""
    for token in value.split():
        if token.isdigit() and token != "0":
            return int(token)
    return None


def parse_transport_port(transport: str) -> int | None:
    """``RTP/AVP/UDP;unicast;client_port=19000`` -> 19000."""
    for part in transport.split(";"):
        part = part.strip()
        if part.startswith("client_port="):
            first = part.split("=", 1)[1].split("-", 1)[0]
            if first.isdigit():
                return int(first)
    return None
