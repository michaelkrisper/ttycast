import pytest

from ttycast import display


class FakeMethod(display.Method):
    name = "fake"
    describes = "a method that only records what it was told"

    def __init__(self, usable=True, works=True):
        self.usable = usable
        self.works = works
        self.calls = []

    def available(self):
        return self.usable

    def off(self):
        self.calls.append("off")
        return self.works

    def on(self):
        self.calls.append("on")
        return True


def test_pick_takes_the_first_usable_method(monkeypatch):
    good, bad = FakeMethod(), FakeMethod(usable=False)
    monkeypatch.setattr(display, "METHODS", (lambda: bad, lambda: good))
    assert display.pick("auto") is good


def test_pick_returns_none_when_nothing_works(monkeypatch):
    monkeypatch.setattr(display, "METHODS", (lambda: FakeMethod(usable=False),))
    assert display.pick("auto") is None


def test_pick_honours_an_explicit_name(monkeypatch):
    monkeypatch.setitem(display.BY_NAME, "fake", FakeMethod)
    assert display.pick("fake").name == "fake"
    assert display.pick("nonsense") is None


def test_output_power_management_is_preferred_over_dimming():
    """Measured: DPMS off saves 3.6 W more than backlight 0 on the same laptop."""
    assert display.METHODS[0] is display.SwayDpms
    assert display.METHODS[-1] is display.Backlight


def test_restore_all_turns_on_every_available_method(monkeypatch):
    good, absent = FakeMethod(), FakeMethod(usable=False)
    monkeypatch.setattr(display, "METHODS", (lambda: good, lambda: absent))
    assert display.restore_all() == ["fake"]
    assert good.calls == ["on"]
    assert absent.calls == []


def test_nothing_happens_without_a_method():
    power = display.ScreenPower(None)
    assert power.active is False
    power.update(viewers=3, now=0.0)
    power.restore()
    assert power.is_off is False
    assert power.status() == ""


def test_a_viewer_darkens_the_screen():
    method = FakeMethod()
    power = display.ScreenPower(method)
    power.update(viewers=1, now=0.0)
    assert method.calls == ["off"]
    assert power.is_off is True
    assert power.status() == "screen off"


def test_the_screen_is_darkened_only_once():
    method = FakeMethod()
    power = display.ScreenPower(method)
    for tick in range(5):
        power.update(viewers=1, now=float(tick))
    assert method.calls == ["off"]


def test_a_failed_off_is_retried_rather_than_assumed():
    method = FakeMethod(works=False)
    power = display.ScreenPower(method)
    power.update(viewers=1, now=0.0)
    power.update(viewers=1, now=1.0)
    assert method.calls == ["off", "off"]
    assert power.is_off is False


def test_a_brief_disconnect_does_not_flap_the_screen():
    method = FakeMethod()
    power = display.ScreenPower(method, grace=5.0)
    power.update(viewers=1, now=0.0)
    power.update(viewers=0, now=1.0)
    power.update(viewers=0, now=3.0)
    power.update(viewers=1, now=4.0)  # client came back inside the grace period
    power.update(viewers=1, now=20.0)
    assert method.calls == ["off"]
    assert power.is_off is True


def test_the_screen_comes_back_after_the_grace_period():
    method = FakeMethod()
    power = display.ScreenPower(method, grace=5.0)
    power.update(viewers=1, now=0.0)
    power.update(viewers=0, now=1.0)
    assert method.calls == ["off"]  # not yet
    power.update(viewers=0, now=6.5)
    assert method.calls == ["off", "on"]
    assert power.is_off is False


def test_restore_is_idempotent_and_safe_when_never_darkened():
    method = FakeMethod()
    power = display.ScreenPower(method)
    power.restore()
    assert method.calls == []
    power.update(viewers=1, now=0.0)
    power.restore()
    power.restore()
    assert method.calls == ["off", "on"]


@pytest.mark.parametrize(
    ("cls", "expected"),
    [
        (display.Backlight, ["brightnessctl", "-q", "-s", "set", "0"]),
        (display.SwayDpms, ["swaymsg", "output", "*", "dpms", "off"]),
        (display.Wlopm, ["wlopm", "--off", "*"]),
        (display.XsetDpms, ["xset", "dpms", "force", "off"]),
    ],
)
def test_off_commands(monkeypatch, cls, expected):
    seen = []
    monkeypatch.setattr(display, "_run", lambda cmd, timeout=5.0: seen.append(cmd) or True)
    cls().off()
    assert seen == [expected]


def test_backlight_restores_through_the_saved_state_file(monkeypatch):
    seen = []
    monkeypatch.setattr(display, "_run", lambda cmd, timeout=5.0: seen.append(cmd) or True)
    display.Backlight().on()
    assert seen == [["brightnessctl", "-q", "-r"]]


def test_methods_are_unavailable_without_their_tool(monkeypatch):
    monkeypatch.setattr(display.shutil, "which", lambda name: None)
    monkeypatch.delenv("DISPLAY", raising=False)
    for cls in display.METHODS:
        assert cls().available() is False
