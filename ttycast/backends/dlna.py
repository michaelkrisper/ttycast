"""Push the stream to a DLNA MediaRenderer (most smart TVs).

The TV is handed a URL into our own HTTP server and pulls the transport stream
itself. Expect one to several seconds of latency: renderers buffer, and that is
not something a sender can turn off. For interactive typing that is noticeable;
for watching a build or a log it is fine.

If discovery comes back empty, the usual cause is the TV: content/screen
sharing has to be enabled in its settings before the renderer is announced.
"""

from __future__ import annotations

import contextlib

from ttycast import upnp
from ttycast.backends.base import Backend, BackendError, Context
from ttycast.encoder import NoEncoderError, have_ffmpeg, pick_encoder


class DlnaBackend(Backend):
    name = "dlna"
    summary = "find a DLNA renderer and hand it the stream (works on most smart TVs)"
    wants_ts = True

    def __init__(self) -> None:
        self.renderer: upnp.Renderer | None = None

    def preflight(self, ctx: Context) -> list[str]:
        problems = []
        if not have_ffmpeg():
            problems.append("ffmpeg not found on PATH (needed to produce the H.264 stream)")
        elif pick_encoder(ctx.options.get("encoder")) is None:
            problems.append(str(NoEncoderError()))
        return problems

    def _pick(self, ctx: Context) -> upnp.Renderer:
        wanted = ctx.options.get("target_name")
        address = ctx.options.get("target_host")
        timeout = float(ctx.options.get("discovery_timeout", 3.0))

        ctx.log("searching for a DLNA renderer ...")
        renderers = upnp.discover(timeout=timeout)
        if not renderers:
            raise BackendError(
                "no DLNA renderer answered. Switch the TV on and enable content/screen "
                "sharing in its settings, then try again (ttycast discover)."
            )
        for renderer in renderers:
            if address and renderer.host != address:
                continue
            if wanted and wanted.lower() not in renderer.name.lower():
                continue
            return renderer
        names = ", ".join(f"{r.name} ({r.host})" for r in renderers)
        raise BackendError(f"no renderer matched. Found: {names}")

    def start(self, ctx: Context) -> None:
        server, ts = ctx.server, ctx.ts
        assert server is not None and ts is not None
        renderer = self._pick(ctx)
        self.renderer = renderer

        ts.start()  # type: ignore[attr-defined]
        url = f"{server.base_url(host=None)}/live.ts"  # type: ignore[attr-defined]
        ctx.log(f"renderer: {renderer.name} ({renderer.host})")
        ctx.log(f"handing over: {url}")
        try:
            upnp.play(renderer, url, title=ctx.options.get("title", "ttycast"))
        except OSError as exc:
            raise BackendError(f"renderer refused the stream: {exc}") from exc

    def on_frame(self, ctx: Context, changed: bool) -> None:
        ts = ctx.ts
        if ts is None:
            return
        _, frame = ctx.server.bus.latest()  # type: ignore[attr-defined]
        if frame is not None:
            ts.write_frame(frame)  # type: ignore[attr-defined]

    def status(self, ctx: Context) -> str:
        if self.renderer is None:
            return "not connected"
        clients = ctx.ts.client_count if ctx.ts else 0  # type: ignore[attr-defined]
        return f"{self.renderer.name} <- {clients} conn"

    def stop(self, ctx: Context) -> None:
        if self.renderer is not None:
            # TV already gone or switched input; nothing to do about it.
            with contextlib.suppress(OSError):
                upnp.stop(self.renderer)
