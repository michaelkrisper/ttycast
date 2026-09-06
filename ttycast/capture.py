"""Read a tmux pane as a cell grid.

tmux is the capture source rather than a screen grabber because the pane is
already text: a grid plus SGR attributes is a few kilobytes, needs no compositor
permission, and re-renders crisply at any TV resolution.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass

from ttycast.ansi import Screen, parse

#: One tmux call has to answer geometry, cursor and identity, so the format is
#: assembled here and split on the way back.
_INFO_FORMAT = "#{pane_id}\t#{pane_width}\t#{pane_height}\t#{cursor_x}\t#{cursor_y}\t#{pane_title}"


class TmuxError(RuntimeError):
    pass


def _tmux(*args: str) -> str:
    if shutil.which("tmux") is None:
        raise TmuxError("tmux not found on PATH")
    proc = subprocess.run(  # noqa: S603
        ["tmux", *args], capture_output=True, text=True
    )
    if proc.returncode != 0:
        raise TmuxError(f"tmux {' '.join(args)}: {proc.stderr.strip()}")
    return proc.stdout


def inside_tmux() -> bool:
    return bool(os.environ.get("TMUX"))


def own_pane() -> str | None:
    return os.environ.get("TMUX_PANE")


def current_session() -> str:
    return _tmux("display-message", "-p", "#{session_name}").strip()


def ensure_session(name: str) -> str:
    """Return a target for session ``name``, creating it detached if needed."""
    try:
        _tmux("has-session", "-t", f"={name}")
    except TmuxError:
        _tmux("new-session", "-d", "-s", name)
    return f"={name}:"


def resolve_target(spec: str) -> str:
    """Turn a ``--target`` spec into something tmux understands.

    ``auto``    follow the active pane of the current session
    ``self``    this very pane
    ``new``     a dedicated detached ``ttycast`` session
    otherwise   passed to tmux unchanged
    """
    if spec == "self":
        pane = own_pane()
        if pane is None:
            raise TmuxError("not running inside tmux, so there is no 'self' pane")
        return pane
    if spec == "new":
        return ensure_session("ttycast")
    if spec == "auto":
        if not inside_tmux():
            return ensure_session("ttycast")
        return f"={current_session()}:"
    return spec


@dataclass(frozen=True, slots=True)
class PaneInfo:
    pane_id: str
    width: int
    height: int
    cursor: tuple[int, int]
    title: str


def parse_info(line: str) -> PaneInfo:
    pane_id, width, height, cx, cy, title = (line.rstrip("\n").split("\t") + [""] * 6)[:6]
    return PaneInfo(pane_id, int(width), int(height), (int(cx), int(cy)), title)


def pane_info(target: str) -> PaneInfo:
    return parse_info(_tmux("display-message", "-p", "-t", target, _INFO_FORMAT))


def fallback_pane(exclude: str) -> str | None:
    """Active pane of the last-used window, skipping ``exclude``.

    Needed because ``--target auto`` otherwise mirrors ttycast's own output the
    moment ttycast is the active pane.
    """
    try:
        listing = _tmux(
            "list-panes",
            "-s",
            "-F",
            "#{pane_id}\t#{window_last_flag}\t#{pane_active}",
        )
    except TmuxError:
        return None
    candidates = []
    for line in listing.splitlines():
        parts = line.split("\t")
        if len(parts) != 3 or parts[0] == exclude:
            continue
        candidates.append((int(parts[1]), int(parts[2]), parts[0]))
    if not candidates:
        return None
    candidates.sort(reverse=True)
    return candidates[0][2]


class PaneSource:
    """Capture the pane behind ``target`` on demand."""

    def __init__(self, target: str, avoid_self: bool = True) -> None:
        self.target = target
        self.avoid_self = avoid_self
        self._own = own_pane()

    def _effective_target(self) -> str:
        if not self.avoid_self or self._own is None:
            return self.target
        try:
            info = pane_info(self.target)
        except TmuxError:
            return self.target
        if info.pane_id != self._own:
            return self.target
        return fallback_pane(self._own) or self.target

    def read(self) -> tuple[Screen, PaneInfo]:
        target = self._effective_target()
        info = pane_info(target)
        text = _tmux("capture-pane", "-p", "-e", "-J", "-t", target)
        screen = parse(text, info.width, info.height)
        screen.cursor = info.cursor
        return screen, info
