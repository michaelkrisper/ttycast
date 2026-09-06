//! Read a tmux pane as a cell grid.
//!
//! tmux is the capture source rather than a screen grabber because the pane is
//! already text: a grid plus SGR attributes is a few kilobytes, needs no
//! compositor permission, and re-renders crisply at any TV resolution.

use std::process::Command;
use std::sync::OnceLock;

/// One tmux call has to answer geometry, cursor and identity at once.
const INFO_FORMAT: &str =
    "#{pane_id}\t#{pane_width}\t#{pane_height}\t#{cursor_x}\t#{cursor_y}\t#{pane_title}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: String,
    pub width: usize,
    pub height: usize,
    pub cursor: (usize, usize),
    pub title: String,
}

/// Resolved once: a `PATH` walk per capture is milliseconds wasted.
fn tmux_binary() -> Option<&'static str> {
    static BINARY: OnceLock<Option<String>> = OnceLock::new();
    BINARY.get_or_init(|| which("tmux")).as_deref()
}

pub fn which(program: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

pub fn tmux(args: &[&str]) -> Result<String, String> {
    let binary = tmux_binary().ok_or_else(|| "tmux not found on PATH".to_string())?;
    let out = Command::new(binary)
        .args(args)
        .output()
        .map_err(|e| format!("tmux {}: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "tmux {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[must_use]
pub fn inside_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

#[must_use]
pub fn own_pane() -> Option<String> {
    std::env::var("TMUX_PANE").ok()
}

pub fn current_session() -> Result<String, String> {
    Ok(tmux(&["display-message", "-p", "#{session_name}"])?
        .trim()
        .to_string())
}

/// Return a target for session `name`, creating it detached if needed.
pub fn ensure_session(name: &str) -> Result<String, String> {
    if tmux(&["has-session", "-t", &format!("={name}")]).is_err() {
        tmux(&["new-session", "-d", "-s", name])?;
    }
    Ok(format!("={name}:"))
}

/// Turn a `--target` spec into something tmux understands.
///
/// `auto` follows the active pane of the current session, `self` is this pane,
/// `new` is a dedicated detached session, anything else goes to tmux unchanged.
pub fn resolve_target(spec: &str) -> Result<String, String> {
    match spec {
        "self" => own_pane().ok_or_else(|| "not inside tmux, so there is no 'self' pane".into()),
        "new" => ensure_session("ttycast"),
        "auto" => {
            if inside_tmux() {
                Ok(format!("={}:", current_session()?))
            } else {
                ensure_session("ttycast")
            }
        }
        other => Ok(other.to_string()),
    }
}

pub fn parse_info(line: &str) -> Result<PaneInfo, String> {
    let fields: Vec<&str> = line.trim_end_matches('\n').split('\t').collect();
    if fields.len() < 5 {
        return Err(format!("unexpected tmux answer: {line:?}"));
    }
    let number = |s: &str| {
        s.parse::<usize>()
            .map_err(|_| format!("not a number: {s:?}"))
    };
    Ok(PaneInfo {
        pane_id: fields[0].to_string(),
        width: number(fields[1])?,
        height: number(fields[2])?,
        cursor: (number(fields[3])?, number(fields[4])?),
        title: fields.get(5).unwrap_or(&"").to_string(),
    })
}

/// `=work:` -> `=work:!`, tmux's own name for the last-used window.
///
/// Letting tmux resolve this saves a `list-panes` round trip and can never go
/// stale the way a cached pane id would.
#[must_use]
pub fn last_window_target(target: &str) -> Option<String> {
    target.ends_with(':').then(|| format!("{target}!"))
}

/// Active pane of the last-used window, skipping `exclude`.
#[must_use]
pub fn fallback_pane(exclude: &str) -> Option<String> {
    let listing = tmux(&[
        "list-panes",
        "-s",
        "-F",
        "#{pane_id}\t#{window_last_flag}\t#{pane_active}",
    ])
    .ok()?;
    let mut candidates: Vec<(u8, u8, String)> = listing
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 3 || f[0] == exclude {
                return None;
            }
            Some((f[1].parse().ok()?, f[2].parse().ok()?, f[0].to_string()))
        })
        .collect();
    candidates.sort_by(|a, b| b.cmp(a));
    candidates.into_iter().next().map(|c| c.2)
}

/// Geometry, cursor and pane contents from a *single* tmux invocation.
///
/// Spawning tmux costs about as much as rendering a whole frame, so the two
/// commands are chained with `;` and answered by one client.
pub fn capture_bundle(target: &str) -> Result<(PaneInfo, String), String> {
    let out = tmux(&[
        "display-message",
        "-p",
        "-t",
        target,
        INFO_FORMAT,
        ";",
        "capture-pane",
        "-p",
        "-e",
        "-J",
        "-t",
        target,
    ])?;
    let (first, rest) = out.split_once('\n').unwrap_or((out.as_str(), ""));
    Ok((parse_info(first)?, rest.to_string()))
}

pub struct PaneSource {
    pub target: String,
    pub avoid_self: bool,
    own: Option<String>,
}

impl PaneSource {
    #[must_use]
    pub fn new(target: String) -> Self {
        Self {
            target,
            avoid_self: true,
            own: own_pane(),
        }
    }

    /// Pane contents as tmux emitted them, before any parsing.
    ///
    /// The caller compares this text with the previous capture: two identical
    /// strings mean nothing moved, so the parse and the render can be skipped.
    pub fn read_raw(&self) -> Result<(PaneInfo, String), String> {
        let (info, text) = capture_bundle(&self.target)?;
        if self.avoid_self && self.own.as_deref() == Some(info.pane_id.as_str()) {
            // We are the active pane; mirroring ourselves would be a hall of
            // mirrors, so fall back to the pane the user was last in.
            let alternative = last_window_target(&self.target)
                .or_else(|| fallback_pane(self.own.as_deref().unwrap_or("")));
            if let Some(alternative) = alternative {
                // Only one window open: showing ourselves is all we can do.
                if let Ok(found) = capture_bundle(&alternative) {
                    return Ok(found);
                }
            }
        }
        Ok((info, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_info_splits_the_tmux_format() {
        let info = parse_info("%3\t80\t24\t5\t7\tbash\n").unwrap();
        assert_eq!(info.pane_id, "%3");
        assert_eq!((info.width, info.height), (80, 24));
        assert_eq!(info.cursor, (5, 7));
        assert_eq!(info.title, "bash");
    }

    #[test]
    fn parse_info_tolerates_an_empty_title() {
        assert_eq!(parse_info("%0\t10\t2\t0\t0\t").unwrap().title, "");
    }

    #[test]
    fn parse_info_rejects_a_short_answer() {
        assert!(parse_info("%0\t10").is_err());
    }

    #[test]
    fn last_window_target_is_derived_from_a_session_target() {
        assert_eq!(last_window_target("=work:").as_deref(), Some("=work:!"));
        assert_eq!(last_window_target("%3"), None);
    }

    #[test]
    fn resolve_target_passes_unknown_specs_through() {
        assert_eq!(resolve_target("mysession:1.0").unwrap(), "mysession:1.0");
    }

    #[test]
    fn which_finds_a_program_that_exists() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-program-xyz").is_none());
    }
}
