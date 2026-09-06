"""Write frames to disk instead of a TV - for developing and for screenshots."""

from __future__ import annotations

from pathlib import Path

from ttycast.backends.base import Backend, Context


class PreviewBackend(Backend):
    name = "preview"
    summary = "write the rendered frame to a PNG file (no network, for testing)"
    wants_ts = False

    def __init__(self) -> None:
        self.path = Path("ttycast-preview.png")
        self.written = 0

    def start(self, ctx: Context) -> None:
        self.path = Path(ctx.options.get("preview_path", self.path))
        ctx.log(f"writing frames to {self.path}")

    def on_frame(self, ctx: Context, changed: bool) -> None:
        if not changed:
            return
        _, frame = ctx.server.bus.latest()  # type: ignore[attr-defined]
        if frame is None:
            return
        tmp = self.path.with_suffix(self.path.suffix + ".tmp")
        # Pillow infers the format from the suffix, and ".tmp" is not one.
        frame.save(tmp, format="PNG")
        tmp.replace(self.path)  # atomic, so a viewer never sees a half file
        self.written += 1

    def status(self, ctx: Context) -> str:
        return f"{self.path} ({self.written} frames)"
