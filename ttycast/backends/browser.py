"""Serve the terminal at a URL and let something open it.

The lowest-common-denominator backend: no ffmpeg, no discovery, no codec
negotiation. MJPEG plays in every browser, phone and most smart-TV browsers,
and the latency is one frame.
"""

from __future__ import annotations

from ttycast.backends.base import Backend, Context
from ttycast.server import local_ip


class BrowserBackend(Backend):
    name = "browser"
    summary = "serve MJPEG at a URL; open it on the TV, a phone or a laptop"
    wants_ts = False

    def start(self, ctx: Context) -> None:
        server = ctx.server
        assert server is not None
        base = server.base_url()  # type: ignore[attr-defined]
        ctx.log(f"open on the TV:  {base}/")
        ctx.log(f"raw stream:      {base}/stream.mjpg")

    def status(self, ctx: Context) -> str:
        return f"http://{local_ip()}:{ctx.port}/"
