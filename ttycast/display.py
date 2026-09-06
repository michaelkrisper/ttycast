"""Turn the local screen off while something else is watching the stream.

A laptop in the corner of the room does not need its own panel lit. Measured on
an i5-4258U at full brightness: 23.4 W with the screen on, 17.2 W with the
backlight at zero, 13.6 W with the output powered down by the compositor. So
asking the compositor is worth 3.6 W more than dimming, and it hands back some
CPU as well because nothing is composited for a disabled output.

Every method here is reversible and is restored on the way out. If ttycast is
killed outright and never gets to restore anything, ``ttycast screen-on`` undoes
all of them.
"""

from __future__ import annotations

import shutil
import subprocess
from dataclasses import dataclass, field


def _run(command: list[str], timeout: float = 5.0) -> bool:
    try:
        result = subprocess.run(  # noqa: S603
            command, capture_output=True, timeout=timeout
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0


class Method:
    """One way of darkening the screen."""

    name = "none"
    #: Shown by ``ttycast doctor`` so the user knows what would happen.
    describes = ""

    def available(self) -> bool:
        return False

    def off(self) -> bool:
        return False

    def on(self) -> bool:
        return False


class Backlight(Method):
    """Set the panel backlight to zero, remembering the previous level."""

    name = "backlight"
    describes = "brightnessctl: backlight to 0, previous level restored on exit"

    def available(self) -> bool:
        if shutil.which("brightnessctl") is None:
            return False
        # Writing the current value back proves we may write at all without
        # changing anything the user would notice.
        current = self._current()
        return current is not None and _run(["brightnessctl", "-q", "set", str(current)])

    def _current(self) -> int | None:
        try:
            result = subprocess.run(  # noqa: S603
                ["brightnessctl", "get"], capture_output=True, text=True, timeout=5
            )
        except (OSError, subprocess.SubprocessError):
            return None
        value = result.stdout.strip()
        return int(value) if value.isdigit() else None

    def off(self) -> bool:
        # -s writes the current level to a state file, so -r works from any
        # shell afterwards, even if this process never gets to run again.
        return _run(["brightnessctl", "-q", "-s", "set", "0"])

    def on(self) -> bool:
        return _run(["brightnessctl", "-q", "-r"])


class SwayDpms(Method):
    """Power the output down through sway, which cuts more than the backlight."""

    name = "sway"
    describes = "swaymsg: output dpms off (powers the panel down, not just dark)"

    def available(self) -> bool:
        return shutil.which("swaymsg") is not None and _run(["swaymsg", "-t", "get_outputs"])

    def off(self) -> bool:
        return _run(["swaymsg", "output", "*", "dpms", "off"])

    def on(self) -> bool:
        return _run(["swaymsg", "output", "*", "dpms", "on"])


class Wlopm(Method):
    """Generic wlroots output power management, for compositors that are not sway."""

    name = "wlopm"
    describes = "wlopm: wlroots output power off"

    def available(self) -> bool:
        return shutil.which("wlopm") is not None

    def off(self) -> bool:
        return _run(["wlopm", "--off", "*"])

    def on(self) -> bool:
        return _run(["wlopm", "--on", "*"])


class XsetDpms(Method):
    """X11 only; does nothing useful under a Wayland compositor."""

    name = "xset"
    describes = "xset: DPMS off (X11 sessions only)"

    def available(self) -> bool:
        import os

        return shutil.which("xset") is not None and bool(os.environ.get("DISPLAY"))

    def off(self) -> bool:
        return _run(["xset", "dpms", "force", "off"])

    def on(self) -> bool:
        return _run(["xset", "dpms", "force", "on"])


#: Preference order, decided by measurement rather than taste. On an i5-4258U
#: laptop at full brightness the panel draws 23.4 W; backlight 0 brings that to
#: 17.2 W, and a compositor DPMS off to 13.6 W. Powering the output down wins by
#: 3.6 W because the panel electronics stop too and the compositor stops
#: rendering to a disabled output, which also gives the CPU back.
#: Backlight is the fallback: it works anywhere, including where no compositor
#: offers output power management.
METHODS: tuple[type[Method], ...] = (SwayDpms, Wlopm, XsetDpms, Backlight)

BY_NAME = {cls.name: cls for cls in METHODS}


def restore_all() -> list[str]:
    """Undo every darkening this machine understands.

    The escape hatch for ``ttycast screen-on``: if ttycast was killed outright
    it never got to restore anything, and a user looking at a black laptop
    should not have to remember which method was in play.
    """
    restored = []
    for cls in METHODS:
        method = cls()
        if method.available() and method.on():
            restored.append(method.name)
    return restored


def pick(preference: str = "auto") -> Method | None:
    """The best usable method, or the named one if it is usable."""
    if preference not in ("auto", ""):
        cls = BY_NAME.get(preference)
        if cls is None:
            return None
        method = cls()
        return method if method.available() else None
    for cls in METHODS:
        method = cls()
        if method.available():
            return method
    return None


@dataclass
class ScreenPower:
    """Screen state driven by the viewer count, with a grace period.

    An MJPEG client that reconnects would otherwise flap the screen on and off,
    so going dark is immediate but coming back waits ``grace`` seconds.
    """

    method: Method | None
    grace: float = 5.0
    is_off: bool = False
    _idle_since: float | None = field(default=None, repr=False)

    @property
    def active(self) -> bool:
        return self.method is not None

    def update(self, viewers: int, now: float) -> None:
        if self.method is None:
            return
        if viewers > 0:
            self._idle_since = None
            if not self.is_off:
                self.is_off = self.method.off()
            return
        if not self.is_off:
            return
        if self._idle_since is None:
            self._idle_since = now
        elif now - self._idle_since >= self.grace:
            self.method.on()
            self.is_off = False
            self._idle_since = None

    def restore(self) -> None:
        """Always safe to call, including when the screen was never darkened."""
        if self.method is not None and self.is_off:
            self.method.on()
            self.is_off = False
        self._idle_since = None

    def status(self) -> str:
        if self.method is None:
            return ""
        return "screen off" if self.is_off else "screen on"
