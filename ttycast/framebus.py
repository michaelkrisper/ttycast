"""Fan-out point between the capture loop and whoever is watching."""

from __future__ import annotations

import io
import threading

from PIL import Image


class FrameBus:
    """Holds the latest frame and wakes up consumers when it changes.

    Consumers block on :meth:`wait` rather than polling, so an idle terminal
    costs nothing beyond the capture loop itself.
    """

    def __init__(self, jpeg_quality: int = 80) -> None:
        self.jpeg_quality = jpeg_quality
        self._cond = threading.Condition()
        self._frame: Image.Image | None = None
        self._jpeg: bytes | None = None
        self._seq = 0

    @property
    def seq(self) -> int:
        with self._cond:
            return self._seq

    def publish(self, frame: Image.Image) -> None:
        with self._cond:
            self._frame = frame
            self._jpeg = None
            self._seq += 1
            self._cond.notify_all()

    def close(self) -> None:
        """Wake every waiter so consumer threads can notice shutdown."""
        with self._cond:
            self._seq += 1
            self._cond.notify_all()

    def latest(self) -> tuple[int, Image.Image | None]:
        with self._cond:
            return self._seq, self._frame

    def latest_jpeg(self) -> tuple[int, bytes | None]:
        with self._cond:
            if self._jpeg is None and self._frame is not None:
                buf = io.BytesIO()
                self._frame.save(buf, format="JPEG", quality=self.jpeg_quality)
                self._jpeg = buf.getvalue()
            return self._seq, self._jpeg

    def wait(self, last_seq: int, timeout: float = 5.0) -> int:
        """Block until the sequence moves past ``last_seq``; return the new one."""
        with self._cond:
            if self._seq == last_seq:
                self._cond.wait(timeout)
            return self._seq
