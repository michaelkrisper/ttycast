"""The one HTTP endpoint every pull-based backend shares.

Routes:

``/``            a remote-friendly landing page
``/stream.mjpg`` MJPEG, lowest latency, plays in any browser
``/frame.jpg``   the current frame as a single image
``/live.ts``     H.264 in MPEG-TS, what a DLNA renderer is pointed at
``/health``      plain-text status, handy from a second machine
"""

from __future__ import annotations

import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from ttycast.encoder import TSBroadcaster
from ttycast.framebus import FrameBus

BOUNDARY = "ttycastframe"

_PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>ttycast</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
 html,body{{margin:0;height:100%;background:#0c0c0e;color:#dedede;
   font:16px/1.5 system-ui,sans-serif}}
 body{{display:flex;flex-direction:column;align-items:center;justify-content:center}}
 img{{max-width:100vw;max-height:100vh;object-fit:contain;image-rendering:auto}}
 a{{color:#ffb000}}
 footer{{position:fixed;bottom:.5rem;opacity:.5;font-size:.8rem}}
</style></head><body>
<img src="/stream.mjpg" alt="terminal">
<footer>ttycast &middot; <a href="/live.ts">live.ts</a> &middot; {host}</footer>
</body></html>
"""


def local_ip(probe: str = "8.8.8.8") -> str:
    """The address a TV on the LAN would have to reach us on."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.connect((probe, 53))
        return sock.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        sock.close()


class _Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ttycast"
    sys_version = ""

    @property
    def bus(self) -> FrameBus:
        return self.server.bus  # type: ignore[attr-defined]

    @property
    def ts(self) -> TSBroadcaster | None:
        return self.server.ts  # type: ignore[attr-defined]

    def log_message(self, format: str, *args) -> None:  # noqa: A002
        logger = getattr(self.server, "logger", None)
        if logger is not None:
            logger(f"{self.address_string()} {format % args}")

    def do_HEAD(self) -> None:  # noqa: N802
        self._dispatch(head_only=True)

    def do_GET(self) -> None:  # noqa: N802
        self._dispatch(head_only=False)

    def _dispatch(self, head_only: bool) -> None:
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        if path == "/":
            self._page()
        elif path == "/health":
            self._health()
        elif path == "/frame.jpg":
            self._frame()
        elif path == "/stream.mjpg":
            self._mjpeg(head_only)
        elif path == "/live.ts":
            self._ts(head_only)
        else:
            self.send_error(404, "no such stream")

    def _send(self, body: bytes, content_type: str, extra: dict[str, str] | None = None) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for key, value in (extra or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def _page(self) -> None:
        host = self.headers.get("Host", local_ip())
        self._send(_PAGE.format(host=host).encode(), "text/html; charset=utf-8")

    def _health(self) -> None:
        ts = self.ts
        body = (
            f"ttycast ok\nframes={self.bus.seq}\n"
            f"ts_running={bool(ts and ts.running)}\n"
            f"ts_clients={ts.client_count if ts else 0}\n"
        ).encode()
        self._send(body, "text/plain; charset=utf-8")

    def _frame(self) -> None:
        _, jpeg = self.bus.latest_jpeg()
        if jpeg is None:
            self.send_error(503, "no frame yet")
            return
        self._send(jpeg, "image/jpeg")

    def _mjpeg(self, head_only: bool) -> None:
        self.send_response(200)
        self.send_header("Content-Type", f"multipart/x-mixed-replace; boundary={BOUNDARY}")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        if head_only:
            return
        seq = -1
        try:
            while not getattr(self.server, "shutting_down", False):
                seq = self.bus.wait(seq)
                _, jpeg = self.bus.latest_jpeg()
                if jpeg is None:
                    continue
                self.wfile.write(
                    f"--{BOUNDARY}\r\nContent-Type: image/jpeg\r\n"
                    f"Content-Length: {len(jpeg)}\r\n\r\n".encode()
                )
                self.wfile.write(jpeg)
                self.wfile.write(b"\r\n")
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass

    def _ts(self, head_only: bool) -> None:
        ts = self.ts
        if ts is None:
            self.send_error(503, "transport stream not enabled")
            return
        self.send_response(200)
        self.send_header("Content-Type", "video/mpeg")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Accept-Ranges", "none")
        self.send_header("Connection", "close")
        # DLNA renderers key off these two; without them many refuse to start.
        self.send_header("transferMode.dlna.org", "Streaming")
        self.send_header(
            "contentFeatures.dlna.org",
            "DLNA.ORG_OP=00;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=8D500000000000000000000000000000",
        )
        self.end_headers()
        if head_only:
            return
        queue, event = ts.subscribe()
        try:
            while not getattr(self.server, "shutting_down", False):
                if not queue:
                    if not event.wait(timeout=5):
                        continue
                    event.clear()
                while queue:
                    self.wfile.write(queue.popleft())
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass
        finally:
            ts.unsubscribe(queue)


class CastServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(
        self,
        bus: FrameBus,
        ts: TSBroadcaster | None = None,
        host: str = "0.0.0.0",  # noqa: S104 - a TV has to reach us on the LAN
        port: int = 8009,
        logger=None,
    ) -> None:
        super().__init__((host, port), _Handler)
        self.bus = bus
        self.ts = ts
        self.logger = logger
        self.shutting_down = False
        self._thread: threading.Thread | None = None

    @property
    def port(self) -> int:
        return self.server_address[1]

    def base_url(self, host: str | None = None) -> str:
        return f"http://{host or local_ip()}:{self.port}"

    def start(self) -> None:
        self._thread = threading.Thread(target=self.serve_forever, name="ttycast-http", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self.shutting_down = True
        self.bus.close()
        self.shutdown()
        self.server_close()
        if self._thread is not None:
            self._thread.join(timeout=3)
