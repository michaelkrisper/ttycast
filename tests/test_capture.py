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


def bundles(mapping):
    """Stub capture_bundle: target -> (pane_id, text)."""

    def fake(target):
        if target not in mapping:
            raise capture.TmuxError(f"can't find window: {target}")
        pane_id, text = mapping[target]
        return capture.PaneInfo(pane_id, 80, 24, (0, 0), "sh"), text

    return fake


def test_pane_source_avoids_mirroring_itself(monkeypatch):
    """An explicit pane target has no ``!`` form, so this falls back to list-panes."""
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(
        capture, "capture_bundle", bundles({"%1": ("%1", "ttycast"), "%2": ("%2", "shell")})
    )
    monkeypatch.setattr(capture, "fallback_pane", lambda exclude: "%2")
    info, text = capture.PaneSource("%1").read_raw()
    assert (info.pane_id, text) == ("%2", "shell")


def test_pane_source_keeps_the_target_when_it_is_another_pane(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(capture, "capture_bundle", bundles({"=work:": ("%7", "shell")}))
    info, text = capture.PaneSource("=work:").read_raw()
    assert (info.pane_id, text) == ("%7", "shell")


def test_pane_source_stays_put_when_there_is_no_other_pane(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(capture, "capture_bundle", bundles({"%1": ("%1", "ttycast")}))
    monkeypatch.setattr(capture, "fallback_pane", lambda exclude: None)
    info, _ = capture.PaneSource("%1").read_raw()
    assert info.pane_id == "%1"


def test_capture_bundle_uses_one_tmux_invocation(monkeypatch):
    calls = []

    def fake_tmux(*args):
        calls.append(args)
        return "%4\t80\t24\t1\t2\tsh\nline one\nline two"

    monkeypatch.setattr(capture, "_tmux", fake_tmux)
    info, text = capture.capture_bundle("%4")
    assert len(calls) == 1
    assert ";" in calls[0]
    assert "display-message" in calls[0] and "capture-pane" in calls[0]
    assert (info.width, info.height, info.cursor) == (80, 24, (1, 2))
    assert text == "line one\nline two"


def test_read_parses_the_bundle_and_carries_the_cursor(monkeypatch):
    monkeypatch.delenv("TMUX_PANE", raising=False)
    monkeypatch.setattr(
        capture,
        "capture_bundle",
        lambda target: (capture.PaneInfo("%0", 4, 2, (2, 1), "sh"), "ab\ncd"),
    )
    screen, info = capture.PaneSource("%0").read()
    assert screen.cursor == (2, 1)
    assert "".join(c.char for c in screen.rows[0]) == "ab  "
    assert info.pane_id == "%0"


def test_last_window_target_is_derived_from_a_session_target():
    assert capture.last_window_target("=work:") == "=work:!"
    assert capture.last_window_target("%3") is None


def test_pane_source_prefers_the_tmux_last_window_target(monkeypatch):
    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(
        capture, "capture_bundle", bundles({"=work:": ("%1", "ttycast"), "=work:!": ("%5", "work")})
    )
    # list-panes must not be needed at all on this path
    monkeypatch.setattr(
        capture, "fallback_pane", lambda exclude: pytest.fail("should not list panes")
    )
    info, text = capture.PaneSource("=work:").read_raw()
    assert (info.pane_id, text) == ("%5", "work")


def test_pane_source_tolerates_being_the_only_window(monkeypatch):
    def only_self(target):
        if target == "=work:":
            return capture.PaneInfo("%1", 80, 24, (0, 0), "sh"), "ttycast"
        raise capture.TmuxError("no such window")

    monkeypatch.setenv("TMUX_PANE", "%1")
    monkeypatch.setattr(capture, "capture_bundle", only_self)
    info, text = capture.PaneSource("=work:").read_raw()
    assert (info.pane_id, text) == ("%1", "ttycast")
