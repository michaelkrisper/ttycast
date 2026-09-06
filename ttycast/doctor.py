"""``ttycast doctor`` - tell the user what will and will not work here.

Every check answers one question a user would otherwise have to answer by
reading source or by watching something fail halfway through.
"""

from __future__ import annotations

import shutil
import socket
from dataclasses import dataclass

from ttycast import capture, display, render, upnp, wifi
from ttycast.encoder import H264_ENCODERS, have_ffmpeg, pick_encoder


@dataclass(frozen=True, slots=True)
class Check:
    name: str
    ok: bool
    detail: str
    #: A failed check that is not fatal for every backend.
    optional: bool = False


def _port_free(port: int) -> bool:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("0.0.0.0", port))  # noqa: S104
        return True
    except OSError:
        return False
    finally:
        sock.close()


def run(port: int = 8009, discover: bool = True) -> list[Check]:
    checks: list[Check] = []

    tmux = shutil.which("tmux")
    checks.append(Check("tmux", bool(tmux), tmux or "not found - ttycast captures tmux panes"))
    checks.append(
        Check(
            "inside tmux",
            capture.inside_tmux(),
            "yes" if capture.inside_tmux() else "no - ttycast will open its own session",
            optional=True,
        )
    )

    font = render.find_font("monospace")
    checks.append(Check("monospace font", bool(font), font or "fc-match found nothing"))

    checks.append(
        Check("ffmpeg", have_ffmpeg(), shutil.which("ffmpeg") or "not found", optional=True)
    )
    encoder = pick_encoder()
    checks.append(
        Check(
            "h264 encoder",
            encoder is not None,
            encoder or f"none of {', '.join(H264_ENCODERS)} - dlna and miracast need one",
            optional=True,
        )
    )

    from ttycast.server import local_ip

    ip = local_ip()
    checks.append(Check("LAN address", ip != "127.0.0.1", ip))
    checks.append(
        Check(f"port {port}", _port_free(port), "free" if _port_free(port) else "already in use")
    )

    method = display.pick("auto")
    checks.append(
        Check(
            "screen off (--screen-off)",
            method is not None,
            method.describes if method else "no usable method found on this session",
            optional=True,
        )
    )

    usable, lines = wifi.p2p_report()
    checks.append(Check("wifi direct (miracast)", usable, "; ".join(lines), optional=True))

    if discover:
        renderers = upnp.discover(timeout=3.0)
        detail = (
            ", ".join(f"{r.name} ({r.host})" for r in renderers)
            if renderers
            else "none found - switch the TV on and enable content/screen sharing"
        )
        checks.append(Check("dlna renderers", bool(renderers), detail, optional=True))

    return checks


def format_checks(checks: list[Check]) -> str:
    width = max(len(c.name) for c in checks) if checks else 0
    lines = []
    for check in checks:
        mark = "ok  " if check.ok else ("warn" if check.optional else "FAIL")
        lines.append(f"[{mark}] {check.name.ljust(width)}  {check.detail}")
    return "\n".join(lines)
