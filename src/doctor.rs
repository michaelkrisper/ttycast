//! `ttycast doctor` - tell the user what will and will not work here.
//!
//! Every check answers one question a user would otherwise have to answer by
//! reading source or by watching something fail halfway through.

use std::net::TcpListener;
use std::time::Duration;

use crate::capture::{inside_tmux, which};
use crate::encoder::{H264_ENCODERS, have_ffmpeg, pick_encoder};
use crate::server::local_ip;
use crate::{display, fonts, upnp, wifi};

pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// A failed check that is not fatal for every backend.
    pub optional: bool,
}

impl Check {
    fn new(name: &str, ok: bool, detail: String, optional: bool) -> Self {
        Self {
            name: name.to_string(),
            ok,
            detail,
            optional,
        }
    }
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("0.0.0.0", port)).is_ok()
}

#[must_use]
pub fn run(port: u16, discover: bool) -> Vec<Check> {
    let mut checks = Vec::new();

    let tmux = which("tmux");
    checks.push(Check::new(
        "tmux",
        tmux.is_some(),
        tmux.unwrap_or_else(|| "not found - ttycast captures tmux panes".into()),
        false,
    ));
    checks.push(Check::new(
        "inside tmux",
        inside_tmux(),
        if inside_tmux() {
            "yes".into()
        } else {
            "no - ttycast will open its own session".into()
        },
        true,
    ));

    let font = fonts::find("monospace");
    checks.push(Check::new(
        "monospace font",
        font.is_some(),
        font.unwrap_or_else(|| "fc-match found nothing".into()),
        false,
    ));

    checks.push(Check::new(
        "ffmpeg",
        have_ffmpeg(),
        which("ffmpeg").unwrap_or_else(|| "not found".into()),
        true,
    ));
    let encoder = pick_encoder(None);
    checks.push(Check::new(
        "h264 encoder",
        encoder.is_some(),
        encoder.unwrap_or_else(|| {
            format!(
                "none of {} - dlna and miracast need one",
                H264_ENCODERS.join(", ")
            )
        }),
        true,
    ));

    let ip = local_ip();
    checks.push(Check::new("LAN address", ip != "127.0.0.1", ip, false));
    checks.push(Check::new(
        &format!("port {port}"),
        port_free(port),
        if port_free(port) {
            "free".into()
        } else {
            "already in use".into()
        },
        false,
    ));

    let method = display::pick("auto");
    checks.push(Check::new(
        "screen off (--screen-off)",
        method.is_some(),
        method.map_or_else(
            || "no usable method found on this session".into(),
            |m| m.describes().to_string(),
        ),
        true,
    ));

    let (usable, lines) = wifi::p2p_report();
    checks.push(Check::new(
        "wifi direct (miracast)",
        usable,
        lines.join("; "),
        true,
    ));

    if discover {
        let renderers = upnp::discover(Duration::from_secs(3));
        let detail = if renderers.is_empty() {
            "none found - switch the TV on and enable content/screen sharing".to_string()
        } else {
            renderers
                .iter()
                .map(|r| format!("{} ({})", r.name, r.host()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        checks.push(Check::new(
            "dlna renderers",
            !renderers.is_empty(),
            detail,
            true,
        ));
    }

    checks
}

#[must_use]
pub fn format(checks: &[Check]) -> String {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    checks
        .iter()
        .map(|check| {
            let mark = if check.ok {
                "ok  "
            } else if check.optional {
                "warn"
            } else {
                "FAIL"
            };
            format!("[{mark}] {:width$}  {}", check.name, check.detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_lines_up_and_marks_severity() {
        let checks = vec![
            Check::new("short", true, "fine".into(), false),
            Check::new("much longer name", false, "broken".into(), false),
            Check::new("optional", false, "meh".into(), true),
        ];
        let text = format(&checks);
        assert!(text.contains("[ok  ] short"));
        assert!(text.contains("[FAIL] much longer name"));
        assert!(text.contains("[warn] optional"));
        // Names are padded to a common width so the details line up.
        let details: Vec<usize> = text
            .lines()
            .map(|line| line.find("  ").map(|_| line.rfind("  ").unwrap()).unwrap())
            .collect();
        assert_eq!(details.len(), 3);
    }

    #[test]
    fn the_local_checks_always_produce_a_report() {
        let checks = run(0, false);
        assert!(checks.iter().any(|c| c.name == "tmux"));
        assert!(checks.iter().any(|c| c.name == "monospace font"));
        assert!(checks.iter().any(|c| c.name.starts_with("port")));
        assert!(!format(&checks).is_empty());
    }
}
