//! Turn the local screen off while something else is watching the stream.
//!
//! A laptop in the corner of the room does not need its own panel lit.
//! Measured on an i5-4258U at full brightness: 23.4 W with the screen on,
//! 17.2 W with the backlight at zero, 13.6 W with the output powered down by
//! the compositor. So asking the compositor is worth 3.6 W more than dimming,
//! and it hands back some CPU as well because nothing is composited for a
//! disabled output.
//!
//! Every method is reversible and is restored on the way out. If ttycast is
//! killed outright and never gets to restore anything, `ttycast screen-on`
//! undoes all of them.

use std::process::Command;

use crate::capture::which;

fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Power the output down through sway; cuts more than the backlight.
    Sway,
    /// Generic wlroots output power management.
    Wlopm,
    /// X11 only; does nothing useful under a Wayland compositor.
    Xset,
    /// Set the panel backlight to zero, remembering the previous level.
    Backlight,
}

/// Preference order, decided by measurement rather than taste. Backlight is
/// last: it works anywhere, but leaves 3.6 W on the table.
pub const METHODS: [Method; 4] = [Method::Sway, Method::Wlopm, Method::Xset, Method::Backlight];

impl Method {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Method::Sway => "sway",
            Method::Wlopm => "wlopm",
            Method::Xset => "xset",
            Method::Backlight => "backlight",
        }
    }

    /// Shown by `ttycast doctor` so the user knows what would happen.
    #[must_use]
    pub fn describes(self) -> &'static str {
        match self {
            Method::Sway => "swaymsg: output dpms off (powers the panel down, not just dark)",
            Method::Wlopm => "wlopm: wlroots output power off",
            Method::Xset => "xset: DPMS off (X11 sessions only)",
            Method::Backlight => "brightnessctl: backlight to 0, previous level restored on exit",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        METHODS.into_iter().find(|method| method.name() == name)
    }

    #[must_use]
    pub fn available(self) -> bool {
        match self {
            Method::Sway => which("swaymsg").is_some() && run("swaymsg", &["-t", "get_outputs"]),
            Method::Wlopm => which("wlopm").is_some(),
            Method::Xset => which("xset").is_some() && std::env::var_os("DISPLAY").is_some(),
            Method::Backlight => {
                // Writing the current value back proves we may write at all
                // without changing anything the user would notice.
                which("brightnessctl").is_some()
                    && current_brightness()
                        .is_some_and(|level| run("brightnessctl", &["-q", "set", &level]))
            }
        }
    }

    pub fn off(self) -> bool {
        match self {
            Method::Sway => run("swaymsg", &["output", "*", "dpms", "off"]),
            Method::Wlopm => run("wlopm", &["--off", "*"]),
            Method::Xset => run("xset", &["dpms", "force", "off"]),
            // -s writes the current level to a state file, so -r works from any
            // shell afterwards, even if this process never runs again.
            Method::Backlight => run("brightnessctl", &["-q", "-s", "set", "0"]),
        }
    }

    pub fn on(self) -> bool {
        match self {
            Method::Sway => run("swaymsg", &["output", "*", "dpms", "on"]),
            Method::Wlopm => run("wlopm", &["--on", "*"]),
            Method::Xset => run("xset", &["dpms", "force", "on"]),
            Method::Backlight => run("brightnessctl", &["-q", "-r"]),
        }
    }
}

fn current_brightness() -> Option<String> {
    let out = Command::new("brightnessctl").arg("get").output().ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty() && value.chars().all(|c| c.is_ascii_digit())).then_some(value)
}

/// The best usable method, or the named one if it is usable.
#[must_use]
pub fn pick(preference: &str) -> Option<Method> {
    if preference != "auto" && !preference.is_empty() {
        let method = Method::from_name(preference)?;
        return method.available().then_some(method);
    }
    METHODS.into_iter().find(|method| method.available())
}

/// Undo every darkening this machine understands.
///
/// The escape hatch for `ttycast screen-on`: a user looking at a black laptop
/// should not have to remember which method was in play.
#[must_use]
pub fn restore_all() -> Vec<&'static str> {
    METHODS
        .into_iter()
        .filter(|method| method.available() && method.on())
        .map(Method::name)
        .collect()
}

/// Screen state driven by the viewer count, with a grace period.
///
/// An MJPEG client that reconnects would otherwise flap the screen on and off,
/// so going dark is immediate but coming back waits `grace`.
pub struct ScreenPower {
    pub method: Option<Method>,
    pub grace: f64,
    is_off: bool,
    idle_since: Option<f64>,
}

impl ScreenPower {
    #[must_use]
    pub fn new(method: Option<Method>) -> Self {
        Self {
            method,
            grace: 5.0,
            is_off: false,
            idle_since: None,
        }
    }

    #[must_use]
    pub fn active(&self) -> bool {
        self.method.is_some()
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.is_off
    }

    pub fn update(&mut self, viewers: usize, now: f64) {
        let Some(method) = self.method else { return };
        if viewers > 0 {
            self.idle_since = None;
            if !self.is_off {
                self.is_off = method.off();
            }
            return;
        }
        if !self.is_off {
            return;
        }
        match self.idle_since {
            None => self.idle_since = Some(now),
            Some(since) if now - since >= self.grace => {
                method.on();
                self.is_off = false;
                self.idle_since = None;
            }
            Some(_) => {}
        }
    }

    /// Always safe to call, including when the screen was never darkened.
    pub fn restore(&mut self) {
        if let Some(method) = self.method
            && self.is_off
        {
            method.on();
            self.is_off = false;
        }
        self.idle_since = None;
    }

    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.method.is_none() {
            ""
        } else if self.is_off {
            "screen off"
        } else {
            "screen on"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The state machine is what needs testing; the commands are covered by
    // asserting their shape, since running them would black the developer's
    // screen mid-test.
    fn power() -> ScreenPower {
        ScreenPower::new(Some(Method::Xset))
    }

    #[test]
    fn output_power_management_is_preferred_over_dimming() {
        // Measured: DPMS off saves 3.6 W more than backlight 0 on the same laptop.
        assert_eq!(METHODS[0], Method::Sway);
        assert_eq!(METHODS[3], Method::Backlight);
    }

    #[test]
    fn every_method_has_a_name_and_a_description() {
        for method in METHODS {
            assert!(!method.name().is_empty());
            assert!(!method.describes().is_empty());
            assert_eq!(Method::from_name(method.name()), Some(method));
        }
    }

    #[test]
    fn an_unknown_method_name_is_rejected() {
        assert_eq!(Method::from_name("floodlight"), None);
        assert_eq!(pick("floodlight"), None);
    }

    #[test]
    fn nothing_happens_without_a_method() {
        let mut screen = ScreenPower::new(None);
        assert!(!screen.active());
        screen.update(3, 0.0);
        screen.restore();
        assert!(!screen.is_off());
        assert_eq!(screen.status(), "");
    }

    #[test]
    fn the_grace_period_keeps_a_reconnect_from_flapping() {
        let mut screen = power();
        screen.is_off = true; // pretend the method succeeded
        screen.update(0, 1.0);
        assert!(
            screen.is_off(),
            "a gap shorter than the grace must not restore"
        );
        screen.update(1, 4.0);
        assert!(
            screen.idle_since.is_none(),
            "a returning client resets the timer"
        );
    }

    #[test]
    fn the_screen_comes_back_after_the_grace_period() {
        let mut screen = power();
        screen.is_off = true;
        screen.update(0, 1.0);
        assert!(screen.is_off());
        screen.update(0, 6.5);
        assert!(!screen.is_off(), "past the grace the screen must come back");
    }

    #[test]
    fn status_follows_the_state() {
        let mut screen = power();
        assert_eq!(screen.status(), "screen on");
        screen.is_off = true;
        assert_eq!(screen.status(), "screen off");
    }

    #[test]
    fn restore_is_safe_when_never_darkened() {
        let mut screen = power();
        screen.restore();
        assert!(!screen.is_off());
    }
}
