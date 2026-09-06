import pytest

from ttycast import capture


def test_parse_info_splits_the_tmux_format():
    info = capture.parse_info("%3\t80\t24\t5\t7\tbash\n")
    assert info.pane_id == "%3"
    assert (info.width, info.height) == (80, 24)
    assert info.cursor == (5, 7)
    assert info.title == "bash"


def test_parse_info_tolerates_an_empty_title():
    info = capture.parse_info("%0\t10\t2\t0\t0\t")
    assert info.title == ""


def test_resolve_target_passes_unknown_specs_through():
    assert capture.resolve_target("mysession:1.0") == "mysession:1.0"


def test_resolve_target_self_uses_the_environment(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%9")
    assert capture.resolve_target("self") == "%9"


def test_resolve_target_self_fails_outside_tmux(monkeypatch):
    monkeypatch.delenv("TMUX_PANE", raising=False)
    with pytest.raises(capture.TmuxError):
        capture.resolve_target("self")


def test_resolve_target_auto_follows_the_session(monkeypatch):
    monkeypatch.setenv("TMUX", "/tmp/tmux-1000/default,1,0")
    monkeypatch.setattr(capture, "current_session", lambda: "work")
    assert capture.resolve_target("auto") == "=work:"


def test_resolve_target_auto_creates_a_session_outside_tmux(monkeypatch):
    monkeypatch.delenv("TMUX", raising=False)
    monkeypatch.setattr(capture, "ensure_session", lambda name: f"={name}:")
    assert capture.resolve_target("auto") == "=ttycast:"


def test_pane_source_avoids_mirroring_itself(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(
        capture, "pane_info", lambda target: capture.PaneInfo("%1", 80, 24, (0, 0), "ttycast")
    )
    monkeypatch.setattr(capture, "fallback_pane", lambda exclude: "%2")
    source = capture.PaneSource("=work:")
    assert source._effective_target() == "%2"


def test_pane_source_keeps_the_target_when_it_is_another_pane(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(
        capture, "pane_info", lambda target: capture.PaneInfo("%7", 80, 24, (0, 0), "bash")
    )
    source = capture.PaneSource("=work:")
    assert source._effective_target() == "=work:"


def test_pane_source_stays_put_when_there_is_no_other_pane(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(
        capture, "pane_info", lambda target: capture.PaneInfo("%1", 80, 24, (0, 0), "ttycast")
    )
    monkeypatch.setattr(capture, "fallback_pane", lambda exclude: None)
    source = capture.PaneSource("=work:")
    assert source._effective_target() == "=work:"
