"""What a backend is: a way of getting the stream in front of a TV.

Capture, rendering and the HTTP endpoint are shared. A backend only decides how
the display learns about the stream - by being told a URL (browser), by being
handed one over UPnP (dlna), or by a wireless display session (miracast).
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Context:
    """Everything a backend may need, filled in by the CLI before ``start``."""

    width: int
    height: int
    fps: int
    bitrate: str
    port: int
    server: object = None  # ttycast.server.CastServer, set once bound
    ts: object = None  # ttycast.encoder.TSBroadcaster, when the backend wants one
    options: dict = field(default_factory=dict)
    log = staticmethod(lambda message: None)


class Backend:
    """Base class; subclasses override what they actually need."""

    #: CLI name, must be unique.
    name = "base"
    #: One-liner for ``--help`` and ``ttycast backends``.
    summary = ""
    #: Whether the shared HTTP server should run an ffmpeg/MPEG-TS pipeline.
    wants_ts = False

    def preflight(self, ctx: Context) -> list[str]:
        """Return blocking problems; empty means good to go."""
        return []

    def start(self, ctx: Context) -> None:
        """Called once the server is up and the first frame exists."""

    def on_frame(self, ctx: Context, changed: bool) -> None:
        """Called for every tick of the capture loop."""

    def status(self, ctx: Context) -> str:
        """Short line for the terminal status bar."""
        return ""

    def stop(self, ctx: Context) -> None:
        """Always called, including after a failed ``start``."""


class BackendError(RuntimeError):
    """A backend could not do its job; the CLI prints this and exits."""
