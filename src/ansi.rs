//! Parse the SGR-decorated text that `tmux capture-pane -e` emits into a cell grid.
//!
//! Only SGR (`ESC [ ... m`) is interpreted; every other escape sequence is
//! skipped, because `capture-pane` renders a settled screen and never emits
//! cursor motion.

/// `Default` means "whatever the theme says", `Indexed` is an xterm palette
/// entry and `Rgb` is a truecolor value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Style {
    /// Bake `reverse` into the colours so the renderer does not have to care.
    #[must_use]
    pub fn resolved(self) -> Self {
        if !self.reverse {
            return self;
        }
        Self {
            fg: self.bg,
            bg: self.fg,
            reverse: false,
            ..self
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub rows: Vec<Vec<Cell>>,
    pub cursor: Option<(usize, usize)>,
}

/// Fold one SGR parameter list into `style`.
pub fn apply_sgr(mut style: Style, params: &[u16]) -> Style {
    if params.is_empty() {
        return Style::default();
    }
    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        i += 1;
        match p {
            0 => style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.reverse = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.reverse = false,
            30..=37 => style.fg = Color::Indexed((p - 30) as u8),
            38 => {
                let (color, next) = extended(params, i);
                style.fg = color;
                i = next;
            }
            39 => style.fg = Color::Default,
            40..=47 => style.bg = Color::Indexed((p - 40) as u8),
            48 => {
                let (color, next) = extended(params, i);
                style.bg = color;
                i = next;
            }
            49 => style.bg = Color::Default,
            90..=97 => style.fg = Color::Indexed((p - 90 + 8) as u8),
            100..=107 => style.bg = Color::Indexed((p - 100 + 8) as u8),
            _ => {}
        }
    }
    style
}

/// Read a `38;5;n` or `38;2;r;g;b` colour starting at `i`.
fn extended(params: &[u16], i: usize) -> (Color, usize) {
    let at = |k: usize| params.get(k).copied().unwrap_or(0) as u8;
    match params.get(i) {
        Some(5) => (Color::Indexed(at(i + 1)), i + 2),
        Some(2) => (Color::Rgb(at(i + 1), at(i + 2), at(i + 3)), i + 4),
        _ => (Color::Default, i + 1),
    }
}

/// Turn captured pane text into a fixed `width` x `height` cell grid.
#[must_use]
pub fn parse(text: &str, width: usize, height: usize) -> Screen {
    let mut rows: Vec<Vec<Cell>> = Vec::with_capacity(height);
    let mut style = Style::default();
    let mut params: Vec<u16> = Vec::with_capacity(8);

    for line in text.split('\n').take(height) {
        let mut row: Vec<Cell> = Vec::with_capacity(width);
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\x1b' => match chars.peek() {
                    Some('[') => {
                        chars.next();
                        params.clear();
                        let mut value: u32 = 0;
                        let mut seen_digit = false;
                        let final_byte = loop {
                            match chars.next() {
                                None => break 'm', // truncated; treat as harmless
                                Some(c @ ('0'..='9')) => {
                                    value = value * 10 + (c as u32 - '0' as u32);
                                    seen_digit = true;
                                }
                                // Sub-parameters (38:2:r:g:b) flatten into the
                                // same list, which is close enough for SGR.
                                Some(';' | ':') => {
                                    params.push(value.min(u32::from(u16::MAX)) as u16);
                                    value = 0;
                                    seen_digit = false;
                                }
                                Some(c) if c.is_ascii_alphabetic() => {
                                    if seen_digit || !params.is_empty() {
                                        params.push(value.min(u32::from(u16::MAX)) as u16);
                                    }
                                    break c;
                                }
                                Some(_) => {}
                            }
                        };
                        if final_byte == 'm' {
                            style = apply_sgr(style, &params);
                        }
                    }
                    Some(']') => {
                        // OSC runs until BEL or ST; neither belongs on screen.
                        for c in chars.by_ref() {
                            if c == '\x07' || c == '\x1b' {
                                break;
                            }
                        }
                    }
                    _ => {
                        chars.next();
                    }
                },
                '\r' => {}
                '\t' => {
                    let stop = 8 - row.len() % 8;
                    for _ in 0..stop {
                        row.push(Cell { ch: ' ', style });
                    }
                }
                _ => row.push(Cell { ch, style }),
            }
        }
        row.truncate(width);
        row.resize(width, Cell { ch: ' ', style });
        rows.push(row);
    }

    while rows.len() < height {
        rows.push(vec![Cell::default(); width]);
    }

    Screen {
        width,
        height,
        rows,
        cursor: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(screen: &Screen, row: usize) -> String {
        screen.rows[row].iter().map(|c| c.ch).collect()
    }

    #[test]
    fn plain_text_is_padded_to_the_grid() {
        let screen = parse("hi", 5, 2);
        assert_eq!(screen.width, 5);
        assert_eq!(text_of(&screen, 0), "hi   ");
        assert_eq!(text_of(&screen, 1), "     ");
    }

    #[test]
    fn long_lines_are_cut_to_the_grid() {
        assert_eq!(text_of(&parse("abcdefgh", 3, 1), 0), "abc");
    }

    #[test]
    fn basic_colors() {
        let screen = parse("\x1b[31mred\x1b[0mplain", 8, 1);
        assert_eq!(screen.rows[0][0].style.fg, Color::Indexed(1));
        assert_eq!(screen.rows[0][3].style, Style::default());
    }

    #[test]
    fn bright_colors_map_to_the_upper_eight() {
        assert_eq!(
            parse("\x1b[91mx", 1, 1).rows[0][0].style.fg,
            Color::Indexed(9)
        );
    }

    #[test]
    fn indexed_and_truecolor() {
        let screen = parse("\x1b[38;5;196ma\x1b[48;2;10;20;30mb", 2, 1);
        assert_eq!(screen.rows[0][0].style.fg, Color::Indexed(196));
        assert_eq!(screen.rows[0][1].style.bg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn subparameter_form_is_accepted() {
        assert_eq!(
            parse("\x1b[38:2:1:2:3mx", 1, 1).rows[0][0].style.fg,
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn attributes_toggle_independently() {
        let style = apply_sgr(Style::default(), &[1, 4, 7]);
        assert!(style.bold && style.underline && style.reverse);
        let style = apply_sgr(style, &[22]);
        assert!(!style.bold);
        assert!(style.underline);
    }

    #[test]
    fn reset_clears_everything() {
        let style = apply_sgr(
            Style {
                fg: Color::Indexed(3),
                bold: true,
                ..Style::default()
            },
            &[0],
        );
        assert_eq!(style, Style::default());
    }

    #[test]
    fn reverse_is_baked_into_colors() {
        let style = Style {
            fg: Color::Indexed(1),
            bg: Color::Indexed(2),
            reverse: true,
            ..Style::default()
        }
        .resolved();
        assert_eq!(style.fg, Color::Indexed(2));
        assert_eq!(style.bg, Color::Indexed(1));
        assert!(!style.reverse);
    }

    #[test]
    fn non_sgr_sequences_are_dropped() {
        assert_eq!(text_of(&parse("a\x1b[2Kb", 3, 1), 0), "ab ");
    }

    #[test]
    fn osc_sequences_are_dropped() {
        assert_eq!(text_of(&parse("\x1b]0;window title\x07ok", 2, 1), 0), "ok");
    }

    #[test]
    fn style_survives_a_line_break() {
        assert_eq!(
            parse("\x1b[32ma\nb", 1, 2).rows[1][0].style.fg,
            Color::Indexed(2)
        );
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(text_of(&parse("a\tb", 10, 1), 0), "a       b ");
    }

    #[test]
    fn identical_input_yields_identical_grids() {
        assert_eq!(parse("a", 2, 1), parse("a", 2, 1));
        assert_ne!(parse("a", 2, 1), parse("b", 2, 1));
        assert_ne!(parse("a", 2, 1), parse("\x1b[31ma", 2, 1));
    }

    #[test]
    fn a_truncated_escape_does_not_hang() {
        assert_eq!(text_of(&parse("ok\x1b[3", 4, 1), 0), "ok  ");
    }
}
