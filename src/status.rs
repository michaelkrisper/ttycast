//! Render tmux's own status line, which `capture-pane` cannot see.
//!
//! The status line belongs to no pane - tmux draws it itself - so it never
//! appears in a capture. It can be asked for instead: `#{E:status-format[0]}`
//! expands to the finished line, with real window names and real colours, but
//! still carrying tmux's own `#[fg=...,bold]` style markup and `#[align=...]`
//! layout directives. This module turns that into cells.
//!
//! Layout is modelled as three sections - left, centre, right - which is what
//! tmux's `align=` directives amount to in every default and themed
//! configuration. The `range`, `list` and marker directives are drawing hints
//! for mouse regions and window-list scrolling; they change nothing about the
//! glyphs, so they are skipped.

use crate::ansi::{Cell, Color, Style};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Align {
    Left,
    Centre,
    Right,
}

/// tmux colour syntax: a name, `colour123`, `#rrggbb`, or `default`.
#[must_use]
pub fn parse_color(text: &str) -> Color {
    let text = text.trim();
    if text.is_empty() || text == "default" || text == "terminal" {
        return Color::Default;
    }
    if let Some(hex) = text.strip_prefix('#')
        && hex.len() == 6
        && let Ok(value) = u32::from_str_radix(hex, 16)
    {
        return Color::Rgb(
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        );
    }
    for prefix in ["colour", "color"] {
        if let Some(rest) = text.strip_prefix(prefix)
            && let Ok(index) = rest.parse::<u16>()
            && index < 256
        {
            return Color::Indexed(index as u8);
        }
    }
    let named = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    if let Some(index) = named.iter().position(|name| *name == text) {
        return Color::Indexed(index as u8);
    }
    if let Some(rest) = text.strip_prefix("bright")
        && let Some(index) = named.iter().position(|name| *name == rest)
    {
        return Color::Indexed(index as u8 + 8);
    }
    Color::Default
}

/// Fold one tmux style token (`fg=red`, `bold`, `none`, ...) into `style`.
fn apply_token(style: &mut Style, token: &str) {
    let token = token.trim();
    if let Some(value) = token.strip_prefix("fg=") {
        style.fg = parse_color(value);
    } else if let Some(value) = token.strip_prefix("bg=") {
        style.bg = parse_color(value);
    } else {
        match token {
            "none" => {
                *style = Style {
                    fg: style.fg,
                    bg: style.bg,
                    ..Style::default()
                };
            }
            "bold" => style.bold = true,
            "nobold" => style.bold = false,
            "dim" => style.dim = true,
            "nodim" => style.dim = false,
            "italics" | "italic" => style.italic = true,
            "noitalics" | "noitalic" => style.italic = false,
            "underscore" | "underline" => style.underline = true,
            "nounderscore" | "nounderline" => style.underline = false,
            "reverse" => style.reverse = true,
            "noreverse" => style.reverse = false,
            _ => {}
        }
    }
}

/// A tmux style string such as `bg=#181825,fg=#cdd6f4,bold`.
#[must_use]
pub fn parse_style(text: &str, base: Style) -> Style {
    let mut style = base;
    for token in text.split([',', ' ']) {
        if !token.is_empty() {
            apply_token(&mut style, token);
        }
    }
    style
}

/// Turn one expanded `status-format` string into a row of `width` cells.
#[must_use]
pub fn render_line(markup: &str, width: usize, base: Style) -> Vec<Cell> {
    let mut sections: [Vec<Cell>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut align = Align::Left;
    let mut style = base;
    // `#[default]` means "whatever push-default last declared", which themes
    // use to return to a window's own colours after a marker.
    let mut defaults = vec![base];

    // `#[list=left-marker]<` declares the glyph tmux would draw if the window
    // list were scrolled off the edge. We do not scroll the list, so the
    // marker never applies and its character is dropped.
    let mut skip_marker = false;

    let mut chars = markup.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '#' {
            match chars.peek() {
                // `##` is an escaped hash, and `##[` an escaped directive.
                Some('#') => {
                    chars.next();
                    push(&mut sections, align, '#', style);
                    continue;
                }
                Some('[') => {
                    chars.next();
                    let mut body = String::new();
                    for c in chars.by_ref() {
                        if c == ']' {
                            break;
                        }
                        body.push(c);
                    }
                    for token in body.split([' ', ',']).filter(|t| !t.is_empty()) {
                        match token {
                            "push-default" => defaults.push(style),
                            "pop-default" => {
                                if defaults.len() > 1 {
                                    defaults.pop();
                                }
                            }
                            "default" => style = *defaults.last().unwrap_or(&base),
                            "align=left" => align = Align::Left,
                            "align=centre" | "align=center" => align = Align::Centre,
                            "align=right" => align = Align::Right,
                            // Mouse ranges and window-list bookkeeping draw
                            // nothing.
                            "list=left-marker" | "list=right-marker" => skip_marker = true,
                            _ if token.starts_with("range=")
                                || token.starts_with("list")
                                || token == "norange"
                                || token == "nolist" => {}
                            _ => apply_token(&mut style, token),
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        if skip_marker {
            skip_marker = false;
            continue;
        }
        push(&mut sections, align, ch, style);
    }

    lay_out(sections, width, base)
}

fn push(sections: &mut [Vec<Cell>; 3], align: Align, ch: char, style: Style) {
    let index = match align {
        Align::Left => 0,
        Align::Centre => 1,
        Align::Right => 2,
    };
    sections[index].push(Cell { ch, style });
}

/// Place the three sections across the row, right-most winning on collision.
fn lay_out(sections: [Vec<Cell>; 3], width: usize, base: Style) -> Vec<Cell> {
    let mut row = vec![
        Cell {
            ch: ' ',
            style: base
        };
        width
    ];
    let [left, centre, right] = sections;

    let place = |row: &mut Vec<Cell>, cells: &[Cell], start: usize| {
        for (offset, cell) in cells.iter().enumerate() {
            if let Some(slot) = row.get_mut(start + offset) {
                *slot = *cell;
            }
        }
    };

    place(&mut row, &left, 0);
    if !centre.is_empty() {
        place(&mut row, &centre, width.saturating_sub(centre.len()) / 2);
    }
    if !right.is_empty() {
        place(&mut row, &right, width.saturating_sub(right.len()));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(row: &[Cell]) -> String {
        row.iter().map(|c| c.ch).collect()
    }

    #[test]
    fn colours_cover_every_tmux_spelling() {
        assert_eq!(parse_color("#89b4fa"), Color::Rgb(0x89, 0xb4, 0xfa));
        assert_eq!(parse_color("colour123"), Color::Indexed(123));
        assert_eq!(parse_color("color7"), Color::Indexed(7));
        assert_eq!(parse_color("red"), Color::Indexed(1));
        assert_eq!(parse_color("brightblue"), Color::Indexed(12));
        assert_eq!(parse_color("default"), Color::Default);
        assert_eq!(parse_color("nonsense"), Color::Default);
    }

    #[test]
    fn a_status_style_string_parses() {
        let style = parse_style("bg=#181825,fg=#cdd6f4,bold", Style::default());
        assert_eq!(style.bg, Color::Rgb(0x18, 0x18, 0x25));
        assert_eq!(style.fg, Color::Rgb(0xcd, 0xd6, 0xf4));
        assert!(style.bold);
    }

    #[test]
    fn plain_text_lands_on_the_left_and_the_row_is_padded() {
        let row = render_line("hello", 10, Style::default());
        assert_eq!(text_of(&row), "hello     ");
    }

    #[test]
    fn the_three_sections_are_placed_across_the_width() {
        let row = render_line(
            "#[align=left]L#[align=centre]C#[align=right]R",
            11,
            Style::default(),
        );
        assert_eq!(text_of(&row), "L    C    R");
    }

    #[test]
    fn a_right_section_is_flush_with_the_end() {
        let row = render_line("#[align=right] 23:04 ", 20, Style::default());
        assert!(text_of(&row).ends_with(" 23:04 "));
    }

    #[test]
    fn style_directives_colour_the_cells_that_follow() {
        let row = render_line("a#[fg=red,bold]b", 3, Style::default());
        assert_eq!(row[0].style, Style::default());
        assert_eq!(row[1].style.fg, Color::Indexed(1));
        assert!(row[1].style.bold);
    }

    #[test]
    fn default_returns_to_the_pushed_style() {
        let row = render_line(
            "#[fg=green]#[push-default]#[fg=red]x#[default]y",
            2,
            Style::default(),
        );
        assert_eq!(row[0].style.fg, Color::Indexed(1));
        assert_eq!(row[1].style.fg, Color::Indexed(2));
    }

    #[test]
    fn pop_default_restores_the_outer_default() {
        let row = render_line(
            "#[fg=blue]#[push-default]#[push-default]#[pop-default]#[default]z",
            1,
            Style::default(),
        );
        assert_eq!(row[0].style.fg, Color::Indexed(4));
    }

    #[test]
    fn list_scroll_markers_are_dropped() {
        // tmux declares the marker glyph even when the list is not scrolled.
        let row = render_line(
            "#[list=left-marker]<#[list=right-marker]>ab",
            4,
            Style::default(),
        );
        assert_eq!(text_of(&row), "ab  ");
    }

    #[test]
    fn layout_directives_draw_nothing() {
        let row = render_line(
            "#[range=window|2 list=focus]#[norange]#[nolist]ab",
            4,
            Style::default(),
        );
        assert_eq!(text_of(&row), "ab  ");
    }

    #[test]
    fn a_doubled_hash_is_a_literal_one() {
        assert_eq!(text_of(&render_line("a##b", 4, Style::default())), "a#b ");
        assert_eq!(text_of(&render_line("##[x]", 5, Style::default())), "#[x] ");
    }

    #[test]
    fn content_wider_than_the_row_is_truncated_not_wrapped() {
        let row = render_line("abcdefghij", 4, Style::default());
        assert_eq!(row.len(), 4);
        assert_eq!(text_of(&row), "abcd");
    }

    #[test]
    fn padding_carries_the_base_style_so_the_bar_is_continuous() {
        let base = parse_style("bg=#181825", Style::default());
        let row = render_line("x", 4, base);
        assert_eq!(row[3].style.bg, Color::Rgb(0x18, 0x18, 0x25));
    }

    #[test]
    fn a_real_tmux_line_renders_its_window_names_and_clock() {
        let markup = "#[align=left range=left default]#[push-default]#[pop-default]\
#[norange default]#[list=on align=left]#[list=left-marker]<#[list=right-marker]>#[list=on]\
#[range=window|1 default]#[push-default] one #[pop-default]#[norange default]\
#[range=window|2 list=focus bg=#89b4fa,fg=#1e1e2e,bold]#[push-default] two #[pop-default]\
#[norange list=on default]#[nolist align=right range=right default]#[push-default] 23:04 \
#[pop-default]#[norange default]";
        let base = parse_style("bg=#181825,fg=#cdd6f4", Style::default());
        let row = render_line(markup, 40, base);
        let text = text_of(&row);
        assert!(text.starts_with(" one  two "), "{text:?}");
        assert!(text.ends_with(" 23:04 "), "{text:?}");
        // The focused window keeps the theme's highlight.
        let two = text.find("two").unwrap();
        assert_eq!(row[two].style.bg, Color::Rgb(0x89, 0xb4, 0xfa));
        assert!(row[two].style.bold);
        // ... and the window before it does not.
        let one = text.find("one").unwrap();
        assert_eq!(row[one].style.bg, base.bg);
    }
}
