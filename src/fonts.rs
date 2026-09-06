//! Find and load a monospace face.
//!
//! fontconfig is asked through `fc-match` rather than linked against, which
//! keeps the binary free of C dependencies and works the same everywhere a
//! desktop font stack is installed.

use std::process::Command;

use fontdue::{Font, FontSettings};

/// Ask fontconfig for the file behind a pattern such as `monospace:bold`.
#[must_use]
pub fn find(pattern: &str) -> Option<String> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if path.is_empty() { None } else { Some(path) }
}

fn read(path: &str) -> Result<Font, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read font {path}: {e}"))?;
    Font::from_bytes(bytes, FontSettings::default())
        .map_err(|e| format!("cannot parse font {path}: {e}"))
}

/// Load the regular and bold faces, falling back to fontconfig's choice.
///
/// The bold face is optional: if it cannot be found or parsed, the regular one
/// stands in, which looks flatter but never fails to draw.
pub fn load(regular: Option<&str>, bold: Option<&str>) -> Result<(Font, Font), String> {
    let regular_path = regular
        .map(ToString::to_string)
        .or_else(|| find("monospace"))
        .ok_or_else(|| "no monospace font found (is fontconfig installed?)".to_string())?;
    let regular_font = read(&regular_path)?;

    let bold_path = bold
        .map(ToString::to_string)
        .or_else(|| find("monospace:bold"));
    let bold_font = match bold_path {
        Some(path) if path != regular_path => read(&path)
            .unwrap_or_else(|_| read(&regular_path).expect("the regular face already parsed")),
        _ => read(&regular_path).expect("the regular face already parsed"),
    };
    Ok((regular_font, bold_font))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monospace_font_exists_on_this_machine() {
        let path = find("monospace").expect("fc-match should answer");
        assert!(
            std::path::Path::new(&path).exists(),
            "{path} does not exist"
        );
    }

    #[test]
    fn an_unknown_pattern_still_yields_a_fallback() {
        // fontconfig never fails to match; it substitutes.
        assert!(find("definitely-not-a-font-family").is_some());
    }

    #[test]
    fn loading_gives_two_usable_faces() {
        let (regular, bold) = load(None, None).expect("fonts should load");
        assert!(regular.metrics('M', 20.0).advance_width > 0.0);
        assert!(bold.metrics('M', 20.0).advance_width > 0.0);
    }

    #[test]
    fn a_missing_explicit_font_is_an_error() {
        assert!(load(Some("/nonexistent/font.ttf"), None).is_err());
    }
}
