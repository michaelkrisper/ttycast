import threading
import time
import urllib.error
import urllib.request

import pytest
from PIL import Image

from ttycast.framebus import FrameBus
from ttycast.server import CastServer


@pytest.fixture
def bus():
    return FrameBus()


@pytest.fixture
def server(bus):
    server = CastServer(bus, ts=None, host="127.0.0.1", port=0)
    server.start()
    yield server
    server.stop()


def get(server, path, timeout=5):
    url = f"http://127.0.0.1:{server.port}{path}"
    with urllib.request.urlopen(url, timeout=timeout) as response:  # noqa: S310
        return response.status, response.headers, response.read()


def test_index_page_points_at_the_stream(server):
    status, headers, body = get(server, "/")
    assert status == 200
    assert headers["Content-Type"].startswith("text/html")
    assert b"/stream.mjpg" in body


def test_health_reports_the_frame_count(server, bus):
    bus.publish(Image.new("RGB", (16, 16), (1, 2, 3)))
    _, _, body = get(server, "/health")
    assert b"ttycast ok" in body
    assert b"frames=1" in body


def test_frame_endpoint_is_unavailable_before_the_first_frame(server):
    with pytest.raises(urllib.error.HTTPError) as excinfo:
        get(server, "/frame.jpg")
    assert excinfo.value.code == 503


def test_frame_endpoint_serves_a_jpeg(server, bus):
    bus.publish(Image.new("RGB", (32, 16), (200, 30, 30)))
    status, headers, body = get(server, "/frame.jpg")
    assert status == 200
    assert headers["Content-Type"] == "image/jpeg"
    assert body.startswith(b"\xff\xd8")


def test_unknown_paths_are_404(server):
    with pytest.raises(urllib.error.HTTPError) as excinfo:
        get(server, "/nope")
    assert excinfo.value.code == 404


def test_live_ts_is_503_without_an_encoder(server):
    with pytest.raises(urllib.error.HTTPError) as excinfo:
        get(server, "/live.ts")
    assert excinfo.value.code == 503


def test_mjpeg_delivers_frames_as_they_are_published(server, bus):
    url = f"http://127.0.0.1:{server.port}/stream.mjpg"
    chunks = []

    def read():
        with urllib.request.urlopen(url, timeout=5) as response:  # noqa: S310
            assert "multipart/x-mixed-replace" in response.headers["Content-Type"]
            chunks.append(response.read(200))

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    for shade in (10, 90, 170):
        bus.publish(Image.new("RGB", (32, 16), (shade, shade, shade)))
    reader.join(timeout=5)
    assert chunks and b"--ttycastframe" in chunks[0]


def test_base_url_uses_the_bound_port(server):
    assert f":{server.port}" in server.base_url(host="10.0.0.1")


def test_viewers_start_at_zero(server):
    assert server.viewers == 0


def test_a_streaming_client_counts_as_a_viewer(server, bus):
    bus.publish(Image.new("RGB", (16, 16), (5, 5, 5)))
    url = f"http://127.0.0.1:{server.port}/stream.mjpg"
    seen = []

    def read():
        with urllib.request.urlopen(url, timeout=5) as response:  # noqa: S310
            response.read(100)
            seen.append(server.viewers)

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    for shade in (20, 60, 120):
        bus.publish(Image.new("RGB", (16, 16), (shade, shade, shade)))
    reader.join(timeout=5)
    assert seen == [1]

    # The handler is parked in bus.wait(); it only learns the client is gone
    # when it tries to write, so keep publishing until it unwinds.
    for shade in range(0, 250, 5):
        if server.viewers == 0:
            break
        bus.publish(Image.new("RGB", (16, 16), (shade, shade, shade)))
        time.sleep(0.05)
    assert server.viewers == 0


def test_health_reports_the_viewer_count(server):
    _, _, body = get(server, "/health")
    assert b"viewers=0" in body
