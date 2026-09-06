"""H.264/MPEG-TS encoding through ffmpeg, plus fan-out of the muxed bytes.

Frames go in as raw RGB on stdin and come out as an endless transport stream on
stdout, which is exactly what a DLNA renderer wants to pull over HTTP.
"""

from __future__ import annotations

import functools
import shutil
import subprocess
import threading
from collections import deque

from PIL import Image


def have_ffmpeg() -> bool:
    return shutil.which("ffmpeg") is not None


#: H.264 encoders ttycast knows how to drive, best first. Distributions that
#: ship a patent-free ffmpeg (Fedora's ``ffmpeg-free``) have no libx264, but
#: usually do have libopenh264 or a VAAPI encoder on the GPU.
H264_ENCODERS = ("libx264", "libopenh264", "h264_vaapi", "h264_qsv", "h264_nvenc")


@functools.lru_cache(maxsize=1)
def available_encoders() -> frozenset[str]:
    if not have_ffmpeg():
        return frozenset()
    try:
        out = subprocess.run(  # noqa: S603
            ["ffmpeg", "-hide_banner", "-encoders"], capture_output=True, text=True, timeout=15
        )
    except (OSError, subprocess.SubprocessError):
        return frozenset()
    names = set()
    for line in out.stdout.splitlines():
        fields = line.split()
        if line.startswith(" V") and len(fields) > 1:
            names.add(fields[1])
    return frozenset(names)


def pick_encoder(preferred: str | None = None) -> str | None:
    """The best H.264 encoder present, or ``None`` if there is none."""
    have = available_encoders()
    if preferred:
        return preferred if preferred in have else None
    return next((name for name in H264_ENCODERS if name in have), None)


def _input_args(width: int, height: int, fps: int, encoder: str) -> list[str]:
    args = ["ffmpeg", "-hide_banner", "-loglevel", "error"]
    if encoder == "h264_vaapi":
        # The device has to be opened before the input it will be used for.
        args += ["-vaapi_device", VAAPI_DEVICE]
    return args + [
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-s",
        f"{width}x{height}",
        "-r",
        str(fps),
        "-i",
        "pipe:0",
        "-an",
    ]


VAAPI_DEVICE = "/dev/dri/renderD128"


def _video_args(encoder: str, fps: int, bitrate: str, gop_seconds: int, preset: str) -> list[str]:
    """H.264 tuned for a still picture that changes in bursts.

    The GOP is deliberately short: a receiver that joins mid-stream shows
    nothing until the next keyframe, and a terminal is cheap to re-send.
    """
    args: list[str] = []
    if encoder == "h264_vaapi":
        args += ["-vf", "format=nv12,hwupload"]
    else:
        args += ["-pix_fmt", "yuv420p"]
    args += ["-c:v", encoder]
    if encoder == "libx264":
        # Only libx264 has these; passing them to another encoder is an error.
        args += ["-preset", preset, "-tune", "zerolatency", "-profile:v", "high"]
    args += [
        "-b:v",
        bitrate,
        "-maxrate",
        bitrate,
        "-bufsize",
        bitrate,
        "-g",
        str(max(1, fps * gop_seconds)),
    ]
    return args


class NoEncoderError(RuntimeError):
    """No usable H.264 encoder; the message says what to install."""

    def __init__(self) -> None:
        super().__init__(
            "ffmpeg has no usable H.264 encoder (looked for "
            + ", ".join(H264_ENCODERS)
            + "). On Fedora, ffmpeg-free ships without libx264: install "
            "openh264/libopenh264 or use the GPU encoder h264_vaapi."
        )


def ts_command(
    width: int,
    height: int,
    fps: int,
    bitrate: str = "2M",
    gop_seconds: int = 2,
    preset: str = "veryfast",
    encoder: str = "libx264",
) -> list[str]:
    """Raw RGB on stdin to MPEG-TS on stdout - what a DLNA renderer pulls."""
    return [
        *_input_args(width, height, fps, encoder),
        *_video_args(encoder, fps, bitrate, gop_seconds, preset),
        "-f",
        "mpegts",
        "-muxdelay",
        "0",
        "-muxpreload",
        "0",
        "pipe:1",
    ]


def rtp_command(
    width: int,
    height: int,
    fps: int,
    host: str,
    port: int,
    bitrate: str = "6M",
    gop_seconds: int = 1,
    preset: str = "ultrafast",
    encoder: str = "libx264",
) -> list[str]:
    """Raw RGB on stdin to RTP/MPEG-TS on the wire - what Miracast carries."""
    return [
        *_input_args(width, height, fps, encoder),
        *_video_args(encoder, fps, bitrate, gop_seconds, preset),
        "-f",
        "rtp_mpegts",
        f"rtp://{host}:{port}?pkt_size=1316",
    ]


class _FfmpegProcess:
    """Shared plumbing: feed raw RGB frames to an ffmpeg process."""

    def __init__(self, command: list[str], capture_stdout: bool) -> None:
        self._command = command
        self._capture = capture_stdout
        self._proc: subprocess.Popen[bytes] | None = None

    @property
    def running(self) -> bool:
        return self._proc is not None and self._proc.poll() is None

    def _spawn(self) -> subprocess.Popen[bytes]:
        if not have_ffmpeg():
            raise RuntimeError("ffmpeg not found on PATH")
        return subprocess.Popen(  # noqa: S603
            self._command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE if self._capture else subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def write_frame(self, frame: Image.Image) -> bool:
        """Push one frame; ``False`` once ffmpeg is gone."""
        proc = self._proc
        if proc is None or proc.stdin is None or proc.poll() is not None:
            return False
        try:
            proc.stdin.write(frame.tobytes())
            proc.stdin.flush()
        except (BrokenPipeError, ValueError, OSError):
            return False
        return True

    def _terminate(self) -> None:
        proc, self._proc = self._proc, None
        if proc is None:
            return
        for stream in (proc.stdin, proc.stdout):
            try:
                if stream:
                    stream.close()
            except OSError:
                pass
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


class RtpSender(_FfmpegProcess):
    """Push the stream straight at a sink over RTP - the Miracast transport."""

    def __init__(
        self,
        width: int,
        height: int,
        fps: int,
        host: str,
        port: int,
        bitrate: str = "6M",
        encoder: str | None = None,
    ) -> None:
        chosen = pick_encoder(encoder)
        if chosen is None:
            raise NoEncoderError
        super().__init__(
            rtp_command(width, height, fps, host, port, bitrate, encoder=chosen),
            capture_stdout=False,
        )
        self.host, self.port, self.encoder = host, port, chosen

    def start(self) -> None:
        if not self.running:
            self._proc = self._spawn()

    def stop(self) -> None:
        self._terminate()


class TSBroadcaster:
    """One ffmpeg process, many HTTP clients.

    Clients are served from their own bounded queue; a client that cannot keep
    up is dropped rather than allowed to stall the encoder.
    """

    def __init__(
        self,
        width: int,
        height: int,
        fps: int,
        bitrate: str = "2M",
        chunk: int = 16 * 1024,
        queue_chunks: int = 64,
        encoder: str | None = None,
    ) -> None:
        self.width, self.height, self.fps = width, height, fps
        self.bitrate = bitrate
        self.encoder = encoder
        self.chunk = chunk
        self.queue_chunks = queue_chunks
        self._proc: subprocess.Popen[bytes] | None = None
        self._pump: threading.Thread | None = None
        self._lock = threading.Lock()
        self._clients: list[deque[bytes]] = []
        self._events: list[threading.Event] = []
        self._stopping = False
        self.dropped_clients = 0

    @property
    def running(self) -> bool:
        return self._proc is not None and self._proc.poll() is None

    def start(self) -> None:
        with self._lock:
            if self.running:
                return
            if not have_ffmpeg():
                raise RuntimeError("ffmpeg not found on PATH")
            chosen = pick_encoder(self.encoder)
            if chosen is None:
                raise NoEncoderError
            self._stopping = False
            self._proc = subprocess.Popen(  # noqa: S603
                ts_command(self.width, self.height, self.fps, self.bitrate, encoder=chosen),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
            self._pump = threading.Thread(target=self._drain, name="ttycast-ts", daemon=True)
            self._pump.start()

    def _drain(self) -> None:
        proc = self._proc
        assert proc is not None and proc.stdout is not None
        while not self._stopping:
            data = proc.stdout.read(self.chunk)
            if not data:
                break
            with self._lock:
                for queue, event in zip(self._clients, self._events, strict=True):
                    if len(queue) >= self.queue_chunks:
                        queue.clear()
                        self.dropped_clients += 1
                    queue.append(data)
                    event.set()

    def write_frame(self, frame: Image.Image) -> bool:
        """Push one frame; ``False`` once ffmpeg is gone."""
        proc = self._proc
        if proc is None or proc.stdin is None or proc.poll() is not None:
            return False
        try:
            proc.stdin.write(frame.tobytes())
            proc.stdin.flush()
        except (BrokenPipeError, ValueError, OSError):
            return False
        return True

    def subscribe(self) -> tuple[deque[bytes], threading.Event]:
        queue: deque[bytes] = deque()
        event = threading.Event()
        with self._lock:
            self._clients.append(queue)
            self._events.append(event)
        return queue, event

    def unsubscribe(self, queue: deque[bytes]) -> None:
        with self._lock:
            if queue in self._clients:
                index = self._clients.index(queue)
                self._clients.pop(index)
                self._events.pop(index)

    @property
    def client_count(self) -> int:
        with self._lock:
            return len(self._clients)

    def stop(self) -> None:
        self._stopping = True
        proc, self._proc = self._proc, None
        if proc is None:
            return
        for stream in (proc.stdin, proc.stdout):
            try:
                if stream:
                    stream.close()
            except OSError:
                pass
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
        with self._lock:
            for event in self._events:
                event.set()
