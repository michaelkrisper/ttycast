import threading

from PIL import Image

from ttycast.framebus import FrameBus


def frame(shade=0):
    return Image.new("RGB", (8, 8), (shade, shade, shade))


def test_starts_empty():
    bus = FrameBus()
    assert bus.latest() == (0, None)
    assert bus.latest_jpeg() == (0, None)


def test_publish_advances_the_sequence():
    bus = FrameBus()
    bus.publish(frame())
    seq, image = bus.latest()
    assert seq == 1
    assert image is not None
    bus.publish(frame(1))
    assert bus.seq == 2


def test_jpeg_is_encoded_once_per_frame():
    bus = FrameBus()
    bus.publish(frame(5))
    first = bus.latest_jpeg()[1]
    assert first is not None and first.startswith(b"\xff\xd8")
    assert bus.latest_jpeg()[1] is first  # cached, not re-encoded
    bus.publish(frame(200))
    assert bus.latest_jpeg()[1] is not first


def test_wait_returns_immediately_when_the_sequence_moved():
    bus = FrameBus()
    bus.publish(frame())
    assert bus.wait(0, timeout=0.1) == 1


def test_wait_blocks_until_a_frame_arrives():
    bus = FrameBus()
    seen = []

    def waiter():
        seen.append(bus.wait(0, timeout=5))

    thread = threading.Thread(target=waiter)
    thread.start()
    bus.publish(frame())
    thread.join(timeout=5)
    assert seen == [1]


def test_close_wakes_waiters():
    bus = FrameBus()
    seen = []
    thread = threading.Thread(target=lambda: seen.append(bus.wait(0, timeout=5)))
    thread.start()
    bus.close()
    thread.join(timeout=5)
    assert seen == [1]
