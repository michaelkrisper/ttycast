import threading

from ttycast.encoder import TSBroadcaster, rtp_command, ts_command


def test_ts_command_declares_the_raw_input():
    cmd = ts_command(1280, 720, 5)
    assert cmd[0] == "ffmpeg"
    assert "rawvideo" in cmd
    assert "1280x720" in cmd
    assert cmd[cmd.index("-r") + 1] == "5"
    assert cmd[-1] == "pipe:1"


def test_ts_command_muxes_to_mpegts():
    cmd = ts_command(640, 480, 10)
    assert cmd[cmd.index("-f", cmd.index("-c:v")) + 1] == "mpegts"


def test_gop_follows_the_frame_rate():
    assert ts_command(640, 480, 5, gop_seconds=2)[ts_command(640, 480, 5).index("-g") + 1] == "10"
    assert ts_command(640, 480, 1, gop_seconds=1)[ts_command(640, 480, 1).index("-g") + 1] == "1"


def test_bitrate_is_capped_not_just_targeted():
    cmd = ts_command(640, 480, 5, bitrate="3M")
    for flag in ("-b:v", "-maxrate", "-bufsize"):
        assert cmd[cmd.index(flag) + 1] == "3M"


def test_rtp_command_targets_the_sink():
    cmd = rtp_command(1280, 720, 30, "192.168.49.1", 5000)
    assert cmd[-2] == "rtp_mpegts"
    assert cmd[-1].startswith("rtp://192.168.49.1:5000")
    assert "pkt_size=1316" in cmd[-1]  # stay under the wifi MTU


def test_broadcaster_starts_idle():
    ts = TSBroadcaster(64, 64, 5)
    assert ts.running is False
    assert ts.client_count == 0
    assert ts.write_frame.__self__ is ts  # bound, callable before start


def test_subscribers_are_tracked_and_removed():
    ts = TSBroadcaster(64, 64, 5)
    queue, event = ts.subscribe()
    assert ts.client_count == 1
    assert isinstance(event, threading.Event)
    ts.unsubscribe(queue)
    assert ts.client_count == 0


def test_unsubscribing_twice_is_harmless():
    ts = TSBroadcaster(64, 64, 5)
    queue, _ = ts.subscribe()
    ts.unsubscribe(queue)
    ts.unsubscribe(queue)
    assert ts.client_count == 0


def test_stop_without_start_is_harmless():
    TSBroadcaster(64, 64, 5).stop()
