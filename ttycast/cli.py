"""Command line: start with the shell, stop with it.

The server has no daemon and no state on disk. It comes up when ``ttycast``
runs, it goes away when ``ttycast`` exits or is interrupted, and the TV is told
to stop on the way out.
"""

from __future__ import annotations

import argparse
import signal
import sys
import threading
import time

from ttycast import __version__, ansi, capture, doctor, render, upnp
from ttycast.backends import REGISTRY, BackendError, Context, create
from ttycast.encoder import TSBroadcaster
from ttycast.framebus import FrameBus
from ttycast.server import CastServer, local_ip

DEFAULT_PORT = 8009


def parse_size(text: str) -> tuple[int, int]:
    try:
        width, height = text.lower().split("x", 1)
        return int(width), int(height)
    except ValueError:
        raise argparse.ArgumentTypeError(f"expected WIDTHxHEIGHT, got {text!r}") from None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="ttycast",
        description="Cast a terminal to a TV.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="backends:\n"
        + "\n".join(f"  {name:<9} {cls.summary}" for name, cls in REGISTRY.items()),
    )
    parser.add_argument("--version", action="version", version=f"ttycast {__version__}")
    sub = parser.add_subparsers(dest="command")

    run = sub.add_parser("run", help="start casting (default)")
    _add_run_arguments(run)
    _add_run_arguments(parser)  # so that bare `ttycast --backend dlna` works

    check = sub.add_parser("doctor", help="check what works on this machine")
    check.add_argument("--port", type=int, default=DEFAULT_PORT)
    check.add_argument("--no-discover", action="store_true", help="skip the LAN scan")

    found = sub.add_parser("discover", help="list DLNA renderers on the LAN")
    found.add_argument("--timeout", type=float, default=3.0)

    sub.add_parser("backends", help="list available backends")
    return parser


def _add_run_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "-b", "--backend", default="browser", choices=sorted(REGISTRY), help="how to reach the TV"
    )
    parser.add_argument(
        "-t",
        "--target",
        default="auto",
        help="tmux target: auto (active pane), self (this pane), new (own session), "
        "or any tmux target string",
    )
    parser.add_argument("-f", "--fps", type=int, default=10, help="capture rate (default: 10)")
    parser.add_argument(
        "-s", "--size", type=parse_size, default=(1280, 720), help="output resolution"
    )
    parser.add_argument("--bitrate", default="2M", help="H.264 bitrate for dlna/miracast")
    parser.add_argument("--encoder", help="force an ffmpeg H.264 encoder (default: autodetect)")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help="HTTP port to serve on")
    parser.add_argument("--font", help="path to a monospace font file")
    parser.add_argument("--quality", type=int, default=80, help="JPEG quality for MJPEG")
    parser.add_argument("--renderer", help="dlna: pick a renderer by name substring")
    parser.add_argument("--renderer-host", help="dlna: pick a renderer by IP")
    parser.add_argument("--sink-host", help="miracast: IP of an already connected sink")
    parser.add_argument("--sink-port", type=int, default=5000, help="miracast: RTP port")
    parser.add_argument(
        "--preview-path", default="ttycast-preview.png", help="preview: file to write"
    )
    parser.add_argument("--once", action="store_true", help="render a single frame and exit")
    parser.add_argument("-q", "--quiet", action="store_true", help="no status line")


class Session:
    """Owns everything that has to be torn down again."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.stop_event = threading.Event()
        self.backend = create(args.backend)
        self.bus = FrameBus(jpeg_quality=args.quality)
        width, height = args.size
        self.ts = (
            TSBroadcaster(width, height, args.fps, args.bitrate, encoder=args.encoder)
            if self.backend.wants_ts
            else None
        )
        self.server: CastServer | None = None
        self.ctx = Context(
            width=width,
            height=height,
            fps=args.fps,
            bitrate=args.bitrate,
            port=args.port,
            ts=self.ts,
            options={
                "target_name": args.renderer,
                "target_host": args.renderer_host,
                "sink_host": args.sink_host,
                "sink_port": args.sink_port,
                "preview_path": args.preview_path,
                "encoder": args.encoder,
                "title": "ttycast",
            },
        )
        self.ctx.log = self.log  # type: ignore[method-assign]
        self.renderer = render.Renderer(size=args.size, font=args.font)
        self.frames = 0
        self.tick_ms = 0.0

    def log(self, message: str) -> None:
        if not self.args.quiet:
            print(f"  {message}", file=sys.stderr)

    def _bind(self) -> CastServer:
        try:
            return CastServer(self.bus, self.ts, port=self.args.port)
        except OSError as exc:
            raise BackendError(
                f"cannot listen on port {self.args.port}: {exc}. Pick another with --port."
            ) from exc

    def run(self) -> int:
        problems = self.backend.preflight(self.ctx)
        if problems:
            print(f"ttycast: backend '{self.backend.name}' cannot start:", file=sys.stderr)
            for line in problems:
                print(f"  - {line}", file=sys.stderr)
            return 2

        source = capture.PaneSource(capture.resolve_target(self.args.target))
        self.server = self._bind()
        self.ctx.server = self.server
        self.server.start()

        if not self.args.quiet:
            print(
                f"ttycast {__version__}  {self.args.size[0]}x{self.args.size[1]} "
                f"@ {self.args.fps} fps  backend={self.backend.name}",
                file=sys.stderr,
            )

        # One frame before the backend starts, so a TV that connects immediately
        # never sees an empty stream.
        self._tick(source)

        try:
            self.backend.start(self.ctx)
        except BackendError as exc:
            print(f"ttycast: {exc}", file=sys.stderr)
            return 2

        if self.args.once:
            self.backend.on_frame(self.ctx, True)
            return 0

        interval = 1.0 / max(1, self.args.fps)
        next_tick = time.monotonic()
        while not self.stop_event.is_set():
            started = time.perf_counter()
            changed = self._tick(source)
            self.backend.on_frame(self.ctx, changed)
            if changed:
                self.tick_ms = (time.perf_counter() - started) * 1000
            next_tick += interval
            delay = next_tick - time.monotonic()
            if delay < 0:
                next_tick = time.monotonic()  # fell behind; do not spiral
            else:
                self.stop_event.wait(delay)
            if not self.args.quiet:
                self._status()
        return 0

    _last_capture: tuple | None = None

    def _tick(self, source: capture.PaneSource) -> bool:
        try:
            info, text = source.read_raw()
        except capture.TmuxError as exc:
            frame = self.renderer.message("ttycast", ["no pane to mirror", str(exc)])
            self.bus.publish(frame)
            self._last_capture = None
            return True
        # Comparing the raw capture is a string compare; it costs microseconds
        # and saves the parse and the render on every idle tick.
        signature = (text, info.width, info.height, info.cursor)
        if signature == self._last_capture:
            return False
        self._last_capture = signature
        screen = ansi.parse(text, info.width, info.height)
        screen.cursor = info.cursor
        self.bus.publish(self.renderer.render(screen))
        self.frames += 1
        return True

    def _status(self) -> None:
        detail = self.backend.status(self.ctx)
        cost = f"{self.tick_ms:4.1f} ms/frame" if self.tick_ms else ""
        line = f"\r  {self.frames} frames  {cost}  {detail}  (ctrl-c to stop)"
        print(line[:110].ljust(110), end="", file=sys.stderr, flush=True)

    def shutdown(self) -> None:
        self.stop_event.set()
        try:
            self.backend.stop(self.ctx)
        except Exception as exc:  # noqa: BLE001 - teardown must not mask the exit
            print(f"\nttycast: backend teardown: {exc}", file=sys.stderr)
        if self.ts is not None:
            self.ts.stop()
        if self.server is not None:
            self.server.stop()
        if not self.args.quiet:
            print(file=sys.stderr)


def command_discover(timeout: float) -> int:
    renderers = upnp.discover(timeout=timeout)
    if not renderers:
        print(
            "no DLNA renderer answered.\n"
            "Most TVs only announce one while content/screen sharing is enabled in "
            "their settings, and never while they are off.",
            file=sys.stderr,
        )
        return 1
    for renderer in renderers:
        print(f"{renderer.name}\t{renderer.host}\t{renderer.control_url}")
    return 0


def command_backends() -> int:
    for name, cls in REGISTRY.items():
        print(f"{name:<9} {cls.summary}")
    return 0


def command_doctor(port: int, discover: bool) -> int:
    checks = doctor.run(port=port, discover=discover)
    print(doctor.format_checks(checks))
    print(f"\nthis machine is reachable at {local_ip()}")
    return 0 if all(c.ok or c.optional for c in checks) else 1


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.command == "doctor":
        return command_doctor(args.port, not args.no_discover)
    if args.command == "discover":
        return command_discover(args.timeout)
    if args.command == "backends":
        return command_backends()

    try:
        session = Session(args)
    except BackendError as exc:
        print(f"ttycast: {exc}", file=sys.stderr)
        return 2

    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: session.stop_event.set())

    try:
        return session.run()
    except capture.TmuxError as exc:
        print(f"ttycast: {exc}", file=sys.stderr)
        return 2
    finally:
        session.shutdown()


if __name__ == "__main__":
    raise SystemExit(main())
