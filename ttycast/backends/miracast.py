"""Wireless display over Miracast.

This is the only backend that mirrors rather than plays a file, so it is the
only one with sub-second latency - the right choice for actually working at the
TV. The cost is that it needs Wi-Fi Direct in the driver.

ttycast does not reimplement the Wi-Fi Direct link layer. It produces the
picture (H.264 in MPEG-TS over RTP, exactly what Miracast carries) and leaves
peer discovery and association to wpa_supplicant, driven either directly or
through MiracleCast. The RTSP capability negotiation lives in :mod:`ttycast.wfd`.

Status: the RTSP layer and the RTP transport are implemented and unit-tested;
the end-to-end path is unverified because no P2P-capable radio was available
during development. Treat it as experimental and report what your sink does.
"""

from __future__ import annotations

import shutil

from ttycast.backends.base import Backend, BackendError, Context
from ttycast.encoder import NoEncoderError, RtpSender, have_ffmpeg, pick_encoder
from ttycast.wifi import p2p_report

#: Userspace helpers that can drive the Wi-Fi Direct side for us.
LINK_HELPERS = ("miracle-wifid", "gnome-network-displays")


class MiracastBackend(Backend):
    name = "miracast"
    summary = "wireless display over Wi-Fi Direct (lowest latency; needs P2P hardware)"
    wants_ts = False

    def __init__(self) -> None:
        self.sender: RtpSender | None = None

    def preflight(self, ctx: Context) -> list[str]:
        problems: list[str] = []
        if not have_ffmpeg():
            problems.append("ffmpeg not found on PATH (needed to produce the H.264 stream)")
        elif pick_encoder(ctx.options.get("encoder")) is None:
            problems.append(str(NoEncoderError()))

        # A pre-negotiated sink means someone else already did the link layer,
        # so the hardware check does not apply.
        if ctx.options.get("sink_host"):
            return problems

        usable, lines = p2p_report()
        if not usable:
            problems.append("no Wi-Fi Direct capable adapter:")
            problems.extend(f"  {line}" for line in lines)
        if not any(shutil.which(helper) for helper in LINK_HELPERS):
            problems.append(
                "no Wi-Fi Direct helper found. Install MiracleCast (miracle-wifid) or "
                "gnome-network-displays, or pass --sink-host/--sink-port to skip "
                "discovery and send RTP to an already connected sink."
            )
        return problems

    def start(self, ctx: Context) -> None:
        host = ctx.options.get("sink_host")
        port = int(ctx.options.get("sink_port", 5000))
        if not host:
            raise BackendError(
                "automatic Miracast session setup is not wired up yet. Connect the sink "
                "with MiracleCast, then run ttycast with --sink-host <ip> --sink-port <port>."
            )
        self.sender = RtpSender(
            ctx.width, ctx.height, ctx.fps, host, port, ctx.bitrate, ctx.options.get("encoder")
        )
        self.sender.start()
        ctx.log(f"sending RTP/MPEG-TS to {host}:{port}")

    def on_frame(self, ctx: Context, changed: bool) -> None:
        if self.sender is None:
            return
        _, frame = ctx.server.bus.latest()  # type: ignore[attr-defined]
        if frame is not None:
            self.sender.write_frame(frame)

    def status(self, ctx: Context) -> str:
        if self.sender is None:
            return "idle"
        host = ctx.options.get("sink_host", "?")
        return f"rtp -> {host}:{ctx.options.get('sink_port', 5000)}"

    def stop(self, ctx: Context) -> None:
        if self.sender is not None:
            self.sender.stop()
