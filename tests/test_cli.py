import argparse

import pytest

from ttycast import backends, cli


@pytest.fixture
def encoder_present(monkeypatch):
    """Pretend ffmpeg is installed with a working encoder.

    Without this the result depends on the machine running the tests: a CI
    image with no ffmpeg reports a different set of problems than a developer
    laptop that has one.
    """
    for module in ("miracast", "dlna"):
        monkeypatch.setattr(f"ttycast.backends.{module}.have_ffmpeg", lambda: True)
        monkeypatch.setattr(
            f"ttycast.backends.{module}.pick_encoder", lambda preferred=None: "libx264"
        )


def test_every_backend_is_constructible_and_documented():
    for name, cls in backends.REGISTRY.items():
        assert cls.summary, f"{name} has no summary"
        assert backends.create(name).name == name


def test_unknown_backend_names_are_rejected():
    with pytest.raises(backends.BackendError) as excinfo:
        backends.create("beamer")
    assert "beamer" in str(excinfo.value)


def test_parse_size():
    assert cli.parse_size("1920x1080") == (1920, 1080)
    assert cli.parse_size("640X480") == (640, 480)
    with pytest.raises(argparse.ArgumentTypeError):
        cli.parse_size("big")


def test_defaults_are_smooth_but_cheap():
    args = cli.build_parser().parse_args([])
    assert args.fps == 10
    assert args.size == (1280, 720)
    assert args.backend == "browser"


def test_backend_can_be_selected_without_a_subcommand():
    args = cli.build_parser().parse_args(["--backend", "dlna", "--fps", "10"])
    assert (args.backend, args.fps) == ("dlna", 10)


def test_run_subcommand_takes_the_same_options():
    args = cli.build_parser().parse_args(["run", "--backend", "preview"])
    assert args.command == "run"
    assert args.backend == "preview"


def test_doctor_subcommand():
    args = cli.build_parser().parse_args(["doctor", "--no-discover"])
    assert args.command == "doctor"
    assert args.no_discover is True


def test_backends_command_lists_all_of_them(capsys):
    assert cli.command_backends() == 0
    out = capsys.readouterr().out
    for name in backends.REGISTRY:
        assert name in out


def test_discover_explains_an_empty_result(monkeypatch, capsys):
    monkeypatch.setattr(cli.upnp, "discover", lambda timeout: [])
    assert cli.command_discover(0.1) == 1
    assert "content/screen sharing" in capsys.readouterr().err


def test_discover_prints_what_it_found(monkeypatch, capsys):
    renderer = cli.upnp.Renderer(
        "Smart TV", "http://1.2.3.4:80/d.xml", "http://1.2.3.4/c", "1.2.3.4"
    )
    monkeypatch.setattr(cli.upnp, "discover", lambda timeout: [renderer])
    assert cli.command_discover(0.1) == 0
    assert "Smart TV" in capsys.readouterr().out


def test_miracast_preflight_blocks_without_p2p(monkeypatch, encoder_present):
    backend = backends.create("miracast")
    monkeypatch.setattr(
        "ttycast.backends.miracast.p2p_report", lambda: (False, ["phy0: no P2P mode"])
    )
    ctx = backends.Context(width=1280, height=720, fps=5, bitrate="2M", port=8009)
    problems = backend.preflight(ctx)
    assert any("Wi-Fi Direct" in problem for problem in problems)


def test_miracast_preflight_skips_hardware_checks_for_a_known_sink(encoder_present):
    backend = backends.create("miracast")
    ctx = backends.Context(
        width=1280,
        height=720,
        fps=5,
        bitrate="2M",
        port=8009,
        options={"sink_host": "192.168.49.1"},
    )
    assert backend.preflight(ctx) == []


def test_browser_backend_needs_nothing():
    ctx = backends.Context(width=1280, height=720, fps=5, bitrate="2M", port=8009)
    assert backends.create("browser").preflight(ctx) == []


def test_dlna_preflight_names_the_missing_encoder(monkeypatch):
    monkeypatch.setattr("ttycast.backends.dlna.have_ffmpeg", lambda: True)
    monkeypatch.setattr("ttycast.backends.dlna.pick_encoder", lambda preferred=None: None)
    ctx = backends.Context(width=1280, height=720, fps=5, bitrate="2M", port=8009)
    problems = backends.create("dlna").preflight(ctx)
    assert len(problems) == 1
    assert "libopenh264" in problems[0]


def test_dlna_preflight_is_happy_with_an_encoder(encoder_present):
    ctx = backends.Context(width=1280, height=720, fps=5, bitrate="2M", port=8009)
    assert backends.create("dlna").preflight(ctx) == []
