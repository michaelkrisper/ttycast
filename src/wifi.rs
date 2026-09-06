//! Can this machine even do Wi-Fi Direct?
//!
//! Miracast rides on Wi-Fi Direct, and Wi-Fi Direct needs a driver that offers
//! `P2P-client`/`P2P-GO` interface modes. Plenty of laptops - anything on
//! Broadcom's proprietary `wl`, for one - never do, and no amount of userspace
//! can work around it. Better to say so up front than to fail deep in a
//! handshake.

use std::process::Command;

use crate::capture::which;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phy {
    pub name: String,
    pub modes: Vec<String>,
    pub driver: String,
}

impl Phy {
    #[must_use]
    pub fn supports_p2p(&self) -> bool {
        self.modes.iter().any(|mode| mode.starts_with("P2P-"))
    }
}

/// Pull `(phy, supported modes)` out of `iw list`.
#[must_use]
pub fn parse_iw_list(text: &str) -> Vec<Phy> {
    let mut phys = Vec::new();
    let mut current: Option<Phy> = None;
    let mut in_modes = false;

    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Wiphy ") {
            if let Some(phy) = current.take() {
                phys.push(phy);
            }
            current = Some(Phy {
                name: name.trim().to_string(),
                modes: Vec::new(),
                driver: String::new(),
            });
            in_modes = false;
            continue;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("Supported interface modes:") {
            in_modes = true;
            continue;
        }
        if in_modes {
            if let Some(mode) = trimmed.strip_prefix("* ") {
                if let Some(phy) = current.as_mut() {
                    phy.modes.push(mode.trim().to_string());
                }
            } else if !trimmed.is_empty() {
                in_modes = false;
            }
        }
    }
    if let Some(phy) = current.take() {
        phys.push(phy);
    }
    phys
}

/// Best-effort driver name via sysfs; empty when it cannot be read.
#[must_use]
pub fn driver_of(phy: &str) -> String {
    std::fs::read_to_string(format!("/sys/class/ieee80211/{phy}/device/uevent"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("DRIVER=").map(ToString::to_string))
        })
        .unwrap_or_default()
}

#[must_use]
pub fn phys() -> Vec<Phy> {
    if which("iw").is_none() {
        return Vec::new();
    }
    let Ok(out) = Command::new("iw").arg("list").output() else {
        return Vec::new();
    };
    let mut found = parse_iw_list(&String::from_utf8_lossy(&out.stdout));
    for phy in &mut found {
        phy.driver = driver_of(&phy.name);
    }
    found
}

/// `(usable, explanation lines)` for the doctor and for preflight.
#[must_use]
pub fn p2p_report_from(found: &[Phy]) -> (bool, Vec<String>) {
    if found.is_empty() {
        return (
            false,
            vec!["no wireless phy found (is 'iw' installed and a wifi card present?)".into()],
        );
    }
    let mut lines = Vec::new();
    let mut usable = false;
    for phy in found {
        let modes = if phy.modes.is_empty() {
            "unknown".to_string()
        } else {
            phy.modes.join(", ")
        };
        let driver = if phy.driver.is_empty() {
            String::new()
        } else {
            format!(" driver={}", phy.driver)
        };
        if phy.supports_p2p() {
            usable = true;
            lines.push(format!("{}{driver}: P2P supported ({modes})", phy.name));
        } else {
            lines.push(format!("{}{driver}: no P2P mode ({modes})", phy.name));
        }
    }
    if !usable {
        lines.push(
            "Miracast needs Wi-Fi Direct. Fix: a USB wifi adapter whose driver offers \
             P2P-GO/P2P-client (mt76 e.g. MT7612U, rtw88, or an Intel AX2xx)."
                .into(),
        );
    }
    (usable, lines)
}

#[must_use]
pub fn p2p_report() -> (bool, Vec<String>) {
    p2p_report_from(&phys())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_P2P: &str = "Wiphy phy0\n\tmax # scan SSIDs: 10\n\tSupported interface modes:\n\
\t\t * IBSS\n\t\t * managed\n\t\t * AP\n\tBand 1:\n";

    const WITH_P2P: &str = "Wiphy phy0\n\tSupported interface modes:\n\t\t * managed\n\
\t\t * AP\n\t\t * P2P-client\n\t\t * P2P-GO\n\t\t * P2P-device\n\tBand 1:\n\
Wiphy phy1\n\tSupported interface modes:\n\t\t * monitor\n";

    #[test]
    fn parse_picks_up_modes() {
        let phys = parse_iw_list(NO_P2P);
        assert_eq!(phys.len(), 1);
        assert_eq!(phys[0].name, "phy0");
        assert_eq!(phys[0].modes, ["IBSS", "managed", "AP"]);
    }

    #[test]
    fn p2p_detection() {
        assert!(!parse_iw_list(NO_P2P)[0].supports_p2p());
        let phys = parse_iw_list(WITH_P2P);
        assert!(phys[0].supports_p2p());
        assert!(!phys[1].supports_p2p());
    }

    #[test]
    fn multiple_phys_are_all_returned() {
        let names: Vec<String> = parse_iw_list(WITH_P2P)
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(names, ["phy0", "phy1"]);
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse_iw_list("").is_empty());
    }

    #[test]
    fn the_report_explains_the_fix_when_p2p_is_missing() {
        let (usable, lines) = p2p_report_from(&parse_iw_list(NO_P2P));
        assert!(!usable);
        assert!(lines.iter().any(|l| l.contains("USB wifi adapter")));
    }

    #[test]
    fn the_report_is_happy_with_a_p2p_adapter() {
        let (usable, lines) = p2p_report_from(&parse_iw_list(WITH_P2P));
        assert!(usable);
        assert!(lines.iter().any(|l| l.contains("P2P supported")));
    }

    #[test]
    fn the_report_handles_no_adapter_at_all() {
        let (usable, lines) = p2p_report_from(&[]);
        assert!(!usable);
        assert!(lines[0].contains("no wireless phy found"));
    }
}
