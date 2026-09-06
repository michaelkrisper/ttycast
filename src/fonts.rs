//! Find and load monospace faces, with a fallback chain for missing glyphs.
//!
//! fontconfig is asked through `fc-match` rather than linked against, which
//! keeps the binary free of C dependencies and works the same everywhere a
//! desktop font stack is installed.
//!
//! A terminal font almost never covers everything on screen: powerline
//! separators, Nerd Font icons and emoji live in other faces. So a character
//! the primary face cannot draw is looked up by codepoint
//! (`fc-match ":charset=e0b0"`), and the answer is cached - the renderer
//! rasterises each character at most once, so this costs one `fc-match` per
//! distinct missing character for the life of the process.

use std::collections::HashMap;
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

/// fontconfig patterns to try for a character, most desirable first.
///
/// A monospace answer is preferred so an icon lands on the cell grid; plain
/// `:charset=` is the catch-all, since fontconfig always substitutes something
/// and the caller has to verify the glyph is really there.
#[must_use]
pub fn fallback_patterns(ch: char) -> [String; 2] {
    let code = format!("{:04x}", ch as u32);
    [
        format!("monospace:charset={code}"),
        format!(":charset={code}"),
    ]
}

fn read(path: &str) -> Result<Font, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read font {path}: {e}"))?;
    Font::from_bytes(bytes, FontSettings::default())
        .map_err(|e| format!("cannot parse font {path}: {e}"))
}

/// The primary faces plus whatever had to be borrowed for missing glyphs.
pub struct FontSet {
    regular: Font,
    bold: Font,
    /// Resolved once per character: an index into `faces`, or `None` when
    /// nothing on the system can draw it.
    resolved: HashMap<char, Option<usize>>,
    faces: Vec<Font>,
    by_path: HashMap<String, usize>,
}

impl FontSet {
    /// Load the regular and bold faces, falling back to fontconfig's choice.
    ///
    /// The bold face is optional: if it cannot be found or parsed, the regular
    /// one stands in, which looks flatter but never fails to draw.
    pub fn load(regular: Option<&str>, bold: Option<&str>) -> Result<Self, String> {
        let regular_path = regular
            .map(ToString::to_string)
            .or_else(|| find("monospace"))
            .ok_or_else(|| "no monospace font found (is fontconfig installed?)".to_string())?;
        let regular_font = read(&regular_path)?;

        let bold_path = bold
            .map(ToString::to_string)
            .or_else(|| find("monospace:bold"));
        let bold_font = match bold_path {
            Some(path) if path != regular_path => read(&path).or_else(|_| read(&regular_path))?,
            _ => read(&regular_path)?,
        };

        Ok(Self {
            regular: regular_font,
            bold: bold_font,
            resolved: HashMap::new(),
            faces: Vec::new(),
            by_path: HashMap::new(),
        })
    }

    /// The face metrics are taken from, and the one that decides the cell size.
    #[must_use]
    pub fn primary(&self) -> &Font {
        &self.regular
    }

    fn load_face(&mut self, path: &str) -> Option<usize> {
        if let Some(index) = self.by_path.get(path) {
            return Some(*index);
        }
        let font = read(path).ok()?;
        let index = self.faces.len();
        self.faces.push(font);
        self.by_path.insert(path.to_string(), index);
        Some(index)
    }

    fn resolve(&mut self, ch: char) -> Option<usize> {
        for pattern in fallback_patterns(ch) {
            let Some(path) = find(&pattern) else { continue };
            let Some(index) = self.load_face(&path) else {
                continue;
            };
            // fontconfig never fails to match, it substitutes - so the answer
            // is only useful if the glyph is actually in there.
            if self.faces[index].has_glyph(ch) {
                return Some(index);
            }
        }
        None
    }

    /// The face that can actually draw `ch`.
    pub fn face(&mut self, ch: char, bold: bool) -> &Font {
        let primary = if bold { &self.bold } else { &self.regular };
        if primary.has_glyph(ch) {
            return if bold { &self.bold } else { &self.regular };
        }
        let index = if let Some(cached) = self.resolved.get(&ch) {
            *cached
        } else {
            let found = self.resolve(ch);
            self.resolved.insert(ch, found);
            found
        };
        match index {
            Some(index) => &self.faces[index],
            // Nothing can draw it; the primary face renders .notdef, which is
            // the honest "this glyph is missing" box.
            None => {
                if bold {
                    &self.bold
                } else {
                    &self.regular
                }
            }
        }
    }

    /// How many extra faces were borrowed so far.
    #[cfg(test)]
    #[must_use]
    pub fn fallback_count(&self) -> usize {
        self.faces.len()
    }
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
        let fonts = FontSet::load(None, None).expect("fonts should load");
        assert!(fonts.primary().metrics('M', 20.0).advance_width > 0.0);
    }

    #[test]
    fn a_missing_explicit_font_is_an_error() {
        assert!(FontSet::load(Some("/nonexistent/font.ttf"), None).is_err());
    }

    #[test]
    fn fallback_patterns_ask_by_codepoint_and_prefer_monospace() {
        let patterns = fallback_patterns('\u{e0b0}');
        assert_eq!(patterns[0], "monospace:charset=e0b0");
        assert_eq!(patterns[1], ":charset=e0b0");
    }

    #[test]
    fn fallback_patterns_pad_short_codepoints() {
        assert_eq!(fallback_patterns('A')[1], ":charset=0041");
    }

    #[test]
    fn ascii_never_needs_a_fallback() {
        let mut fonts = FontSet::load(None, None).unwrap();
        for ch in ['a', 'Z', '0', '#', ' '] {
            fonts.face(ch, false);
        }
        assert_eq!(fonts.fallback_count(), 0);
    }

    #[test]
    fn a_character_no_font_has_does_not_loop_or_panic() {
        let mut fonts = FontSet::load(None, None).unwrap();
        // A private-use codepoint nothing sensible covers; must still answer.
        let face = fonts.face('\u{10fffd}', false);
        assert!(face.metrics('M', 20.0).advance_width > 0.0);
        // And the negative answer is cached rather than re-queried.
        fonts.face('\u{10fffd}', false);
        assert_eq!(fonts.resolved.len(), 1);
    }
}

#[cfg(test)]
mod metric_probe {
    use super::*;

    #[test]
    #[ignore = "reports numbers, asserts nothing"]
    fn advances_against_the_cell() {
        let mut fonts = FontSet::load(None, None).unwrap();
        let px = 34.0;
        let cell_w = fonts.primary().metrics('M', px).advance_width;
        eprintln!("  cell width from 'M' = {cell_w:.2} px at {px} px");
        for ch in [
            'M', '─', '│', '┼', '┐', '█', '\u{e0b0}', '\u{f015}', '\u{28ff}',
        ] {
            let primary_has = fonts.primary().has_glyph(ch);
            let face = fonts.face(ch, false);
            let m = face.metrics(ch, px);
            eprintln!(
                "  U+{:04X} primary={:5} advance={:6.2} bitmap={}x{} xmin={} ymin={}",
                ch as u32, primary_has, m.advance_width, m.width, m.height, m.xmin, m.ymin
            );
        }
    }
}
