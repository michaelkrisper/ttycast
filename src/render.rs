//! Turn a cell grid into an RGB frame sized for a TV.
//!
//! Three things keep this cheap enough for a slow laptop:
//!
//! * every glyph is rasterised once into a coverage mask and then blitted;
//! * consecutive frames are diffed by row, so typing repaints one row;
//! * a run of equally styled cells shares one background fill.
//!
//! The canvas persists between calls, so [`Renderer::render`] copies it out.

use std::collections::HashMap;

use fontdue::Font;

use crate::ansi::{Cell, Color, Screen, Style};
use crate::fonts::FontSet;

pub type Rgb = [u8; 3];

/// xterm's first 16 entries, the ones a terminal theme actually swaps out.
pub const BASE16: [Rgb; 16] = [
    [0, 0, 0],
    [205, 49, 49],
    [13, 188, 121],
    [229, 229, 16],
    [36, 114, 200],
    [188, 63, 188],
    [17, 168, 205],
    [229, 229, 229],
    [102, 102, 102],
    [241, 76, 76],
    [35, 209, 139],
    [245, 245, 67],
    [59, 142, 234],
    [214, 112, 214],
    [41, 184, 219],
    [255, 255, 255],
];

/// The full 256-entry xterm palette: 16 base, a 6x6x6 cube, a grey ramp.
#[must_use]
pub fn xterm256() -> [Rgb; 256] {
    let mut palette = [[0u8; 3]; 256];
    palette[..16].copy_from_slice(&BASE16);
    let levels = [0u8, 95, 135, 175, 215, 255];
    let mut i = 16;
    for &r in &levels {
        for &g in &levels {
            for &b in &levels {
                palette[i] = [r, g, b];
                i += 1;
            }
        }
    }
    for step in 0..24u8 {
        let v = 8 + step * 10;
        palette[i] = [v, v, v];
        i += 1;
    }
    palette
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    /// TVs overscan; a margin keeps the outermost cells on the panel.
    pub margin: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            fg: [222, 222, 222],
            bg: [12, 12, 14],
            cursor: [255, 176, 0],
            // TVs mostly stopped overscanning; a browser never does. Keep a
            // hair of inset so the outermost cells are not flush to the bezel.
            margin: 0.01,
        }
    }
}

#[must_use]
pub fn resolve(color: Color, fallback: Rgb, bold: bool, palette: &[Rgb; 256]) -> Rgb {
    match color {
        Color::Default => fallback,
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(mut index) => {
            if bold && index < 8 {
                index += 8;
            }
            palette[index as usize]
        }
    }
}

/// A rasterised glyph, positioned relative to the cell's top-left corner.
struct Glyph {
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    coverage: Vec<u8>,
}

/// `(start_column, length, style)` for each run of equally styled cells.
#[must_use]
pub fn style_runs(row: &[Cell]) -> Vec<(usize, usize, Style)> {
    let mut runs = Vec::with_capacity(8);
    let mut start = 0;
    let mut current: Option<Style> = None;
    for (col, cell) in row.iter().enumerate() {
        let style = cell.style.resolved();
        match current {
            None => {
                current = Some(style);
                start = col;
            }
            Some(active) if active != style => {
                runs.push((start, col - start, active));
                current = Some(style);
                start = col;
            }
            Some(_) => {}
        }
    }
    if let Some(active) = current {
        runs.push((start, row.len() - start, active));
    }
    runs
}

/// One finished frame, ready to be encoded or served.
#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

pub struct Renderer {
    width: usize,
    height: usize,
    theme: Theme,
    palette: [Rgb; 256],
    fonts: FontSet,
    grid: Option<(usize, usize)>,
    cell_w: usize,
    cell_h: usize,
    origin_x: usize,
    origin_y: usize,
    px: f32,
    ascent: f32,
    glyphs: HashMap<(char, bool), Glyph>,
    canvas: Vec<u8>,
    canvas_valid: bool,
    prev_rows: Option<Vec<Vec<Cell>>>,
    prev_cursor: Option<(usize, usize)>,
    pub full_redraws: u64,
    pub rows_drawn: u64,
}

impl Renderer {
    pub fn new(width: usize, height: usize, theme: Theme, fonts: FontSet) -> Self {
        Self {
            width,
            height,
            theme,
            palette: xterm256(),
            fonts,
            grid: None,
            cell_w: 1,
            cell_h: 1,
            origin_x: 0,
            origin_y: 0,
            px: 12.0,
            ascent: 10.0,
            glyphs: HashMap::new(),
            canvas: vec![0; width * height * 3],
            canvas_valid: false,
            prev_rows: None,
            prev_cursor: None,
            full_redraws: 0,
            rows_drawn: 0,
        }
    }

    /// Exposed for the tests, which assert the grid actually fits the frame.
    #[cfg(test)]
    #[must_use]
    pub fn font_size(&self) -> f32 {
        self.px
    }

    #[cfg(test)]
    #[must_use]
    pub fn cell(&self) -> (usize, usize) {
        (self.cell_w, self.cell_h)
    }

    #[cfg(test)]
    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Cell advance, cell height and the baseline offset within the cell.
    ///
    /// The height comes from FULL BLOCK (U+2588) when the font has it. A font
    /// draws that glyph to fill its em box exactly, which is also what every
    /// box-drawing and block character is designed against - so taking the
    /// cell from it is what makes `+`-corners and `|`-verticals actually join
    /// between rows. Line metrics from `hhea` are a few pixels shorter, and a
    /// vertical bar drawn against them overhangs into the next row, where the
    /// following row's background fill chops it off.
    fn metrics_at(font: &Font, px: f32) -> (usize, usize, f32) {
        let advance = font.metrics('M', px).advance_width;
        let cell_w = advance.ceil().max(1.0) as usize;

        if font.has_glyph('\u{2588}') {
            let block = font.metrics('\u{2588}', px);
            if block.height > 0 {
                // Placing a glyph uses `ascent - (ymin + height)`; making that
                // zero for the block puts its top flush with the cell top.
                let baseline = (block.ymin + block.height as i32) as f32;
                return (cell_w, block.height.max(1), baseline);
            }
        }

        let line = font.horizontal_line_metrics(px);
        let height = line.map_or(px * 1.2, |m| m.ascent - m.descent);
        let baseline = line.map_or(px, |m| m.ascent);
        (cell_w, height.ceil().max(1.0) as usize, baseline)
    }

    /// Largest font size whose `cols` x `rows` grid still fits in the frame.
    fn fit(&self, cols: usize, rows: usize) -> f32 {
        let inset = self.theme.margin;
        let max_w = self.width as f32 * (1.0 - 2.0 * inset);
        let max_h = self.height as f32 * (1.0 - 2.0 * inset);
        let (mut lo, mut hi, mut best) = (4.0f32, 200.0f32, 4.0f32);
        while hi - lo > 0.5 {
            let mid = f32::midpoint(lo, hi);
            let (cw, ch, _) = Self::metrics_at(self.fonts.primary(), mid);
            if (cw * cols) as f32 <= max_w && (ch * rows) as f32 <= max_h {
                best = mid;
                lo = mid;
            } else {
                hi = mid;
            }
        }
        best.floor().max(4.0)
    }

    fn prepare(&mut self, cols: usize, rows: usize) {
        if self.grid == Some((cols, rows)) {
            return;
        }
        self.px = self.fit(cols, rows);
        let (cw, ch, baseline) = Self::metrics_at(self.fonts.primary(), self.px);
        self.cell_w = cw;
        self.cell_h = ch;
        self.ascent = baseline;
        self.origin_x = self.width.saturating_sub(cw * cols) / 2;
        self.origin_y = self.height.saturating_sub(ch * rows) / 2;
        self.grid = Some((cols, rows));
        self.glyphs.clear();
        // Geometry moved, so nothing already on the canvas can be reused.
        self.canvas_valid = false;
    }

    fn glyph(&mut self, ch: char, bold: bool) -> &Glyph {
        let key = (ch, bold);
        if !self.glyphs.contains_key(&key) {
            let px = self.px;
            // The borrow of the font set ends with this block, so the glyph
            // cache can be written to below.
            let (metrics, coverage) = {
                let font = self.fonts.face(ch, bold);
                font.rasterize(ch, px)
            };
            // fontdue reports ymin as the bitmap's bottom relative to the
            // baseline, with y growing upwards; our buffer grows downwards.
            let top = self.ascent.round() as i32 - (metrics.ymin + metrics.height as i32);
            self.glyphs.insert(
                key,
                Glyph {
                    width: metrics.width,
                    height: metrics.height,
                    left: metrics.xmin,
                    top,
                    coverage,
                },
            );
        }
        &self.glyphs[&key]
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Rgb) {
        let stride = self.width * 3;
        let right = (x + w).min(self.width);
        if right <= x {
            return;
        }
        for row in y..(y + h).min(self.height) {
            let start = row * stride + x * 3;
            let end = row * stride + right * 3;
            for pixel in self.canvas[start..end].chunks_exact_mut(3) {
                pixel.copy_from_slice(&color);
            }
        }
    }

    /// Alpha-blend one rasterised glyph into the canvas.
    fn blit(&mut self, ch: char, bold: bool, pen_x: usize, pen_y: usize, color: Rgb) {
        let (gw, gh, gleft, gtop) = {
            let glyph = self.glyph(ch, bold);
            (glyph.width, glyph.height, glyph.left, glyph.top)
        };
        if gw == 0 || gh == 0 {
            return;
        }
        let x0 = pen_x as i32 + gleft;
        let y0 = pen_y as i32 + gtop;
        let stride = self.width * 3;
        // Taking the coverage out avoids borrowing self immutably and mutably
        // at once; it goes straight back afterwards.
        let coverage = {
            let glyph = self.glyphs.get_mut(&(ch, bold)).expect("just inserted");
            std::mem::take(&mut glyph.coverage)
        };
        for gy in 0..gh {
            let y = y0 + gy as i32;
            if y < 0 || y >= self.height as i32 {
                continue;
            }
            let base = y as usize * stride;
            for gx in 0..gw {
                let alpha = u32::from(coverage[gy * gw + gx]);
                if alpha == 0 {
                    continue;
                }
                let x = x0 + gx as i32;
                if x < 0 || x >= self.width as i32 {
                    continue;
                }
                let at = base + x as usize * 3;
                if alpha == 255 {
                    self.canvas[at..at + 3].copy_from_slice(&color);
                } else {
                    let inv = 255 - alpha;
                    for (channel, &src) in self.canvas[at..at + 3].iter_mut().zip(color.iter()) {
                        let dst = u32::from(*channel);
                        *channel = ((u32::from(src) * alpha + dst * inv) / 255) as u8;
                    }
                }
            }
        }
        self.glyphs
            .get_mut(&(ch, bold))
            .expect("still present")
            .coverage = coverage;
    }

    fn draw_row(&mut self, row: &[Cell], index: usize) {
        let cw = self.cell_w;
        let ch = self.cell_h;
        let y = self.origin_y + index * ch;
        let width = self.width;
        self.fill_rect(0, y, width, ch, self.theme.bg);

        for (col, len, style) in style_runs(row) {
            let x = self.origin_x + col * cw;
            let bg = resolve(style.bg, self.theme.bg, false, &self.palette);
            if bg != self.theme.bg {
                self.fill_rect(x, y, cw * len, ch, bg);
            }
            if row[col..col + len].iter().all(|c| c.ch == ' ') {
                continue;
            }
            let mut fg = resolve(style.fg, self.theme.fg, style.bold, &self.palette);
            if style.dim {
                fg = [fg[0] / 2, fg[1] / 2, fg[2] / 2];
            }
            for (i, cell) in row[col..col + len].iter().enumerate() {
                if cell.ch != ' ' {
                    self.blit(cell.ch, style.bold, x + i * cw, y, fg);
                }
            }
            if style.underline {
                self.fill_rect(x, y + ch.saturating_sub(2), cw * len, 1, fg);
            }
        }
        self.rows_drawn += 1;
    }

    fn draw_cursor(&mut self, cursor: (usize, usize), screen: &Screen) {
        let (cx, cy) = cursor;
        if cx >= screen.width || cy >= screen.height {
            return;
        }
        let (cw, ch) = (self.cell_w, self.cell_h);
        let x = self.origin_x + cx * cw;
        let y = self.origin_y + cy * ch;
        let color = self.theme.cursor;
        self.fill_rect(x, y, cw, 2, color);
        self.fill_rect(x, y + ch.saturating_sub(2), cw, 2, color);
        self.fill_rect(x, y, 2, ch, color);
        self.fill_rect(x + cw.saturating_sub(2), y, 2, ch, color);
    }

    /// Rows that differ from the last frame, plus the two the cursor touches.
    fn dirty_rows(&self, screen: &Screen) -> Vec<usize> {
        let previous = self.prev_rows.as_ref().expect("checked by caller");
        let mut dirty: Vec<usize> = (0..screen.height)
            .filter(|&i| screen.rows[i] != previous[i])
            .collect();
        for cursor in [self.prev_cursor, screen.cursor].into_iter().flatten() {
            if cursor.1 < screen.height && !dirty.contains(&cursor.1) {
                dirty.push(cursor.1);
            }
        }
        dirty
    }

    pub fn render(&mut self, screen: &Screen) -> Frame {
        self.prepare(screen.width, screen.height);

        let reusable = self.canvas_valid
            && self
                .prev_rows
                .as_ref()
                .is_some_and(|rows| rows.len() == screen.height);

        let rows: Vec<usize> = if reusable {
            self.dirty_rows(screen)
        } else {
            let bg = self.theme.bg;
            for pixel in self.canvas.chunks_exact_mut(3) {
                pixel.copy_from_slice(&bg);
            }
            self.canvas_valid = true;
            self.full_redraws += 1;
            (0..screen.height).collect()
        };

        for index in rows {
            let row = screen.rows[index].clone();
            self.draw_row(&row, index);
        }

        if let Some(cursor) = screen.cursor {
            self.draw_cursor(cursor, screen);
        }

        self.prev_rows = Some(screen.rows.clone());
        self.prev_cursor = screen.cursor;

        Frame {
            width: self.width,
            height: self.height,
            data: self.canvas.clone(),
        }
    }

    /// A standalone card, used for "waiting for a pane" style states.
    pub fn message(&mut self, title: &str, lines: &[String]) -> Frame {
        // A card is not part of the incremental stream, so the diff state goes.
        self.grid = None;
        self.canvas_valid = false;
        let mut text = vec![title.to_string(), String::new()];
        text.extend(lines.iter().cloned());
        let width = text.iter().map(|l| l.chars().count()).max().unwrap_or(1) + 4;
        let height = text.len() + 2;
        let body = text
            .iter()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut screen = crate::ansi::parse(&format!("\n{body}"), width, height);
        screen.cursor = None;
        self.render(&screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::parse;

    fn renderer(width: usize, height: usize) -> Renderer {
        let fonts = FontSet::load(None, None).expect("a monospace font must exist");
        Renderer::new(width, height, Theme::default(), fonts)
    }

    fn screen_of(lines: &[&str], width: usize) -> Screen {
        parse(&lines.join("\n"), width, lines.len())
    }

    #[test]
    fn palette_has_all_256_entries() {
        let palette = xterm256();
        assert_eq!(palette[..16], BASE16);
        assert_eq!(palette[196], [255, 0, 0]);
        assert_eq!(palette[255], [238, 238, 238]);
    }

    #[test]
    fn resolve_falls_back_to_the_theme_default() {
        let palette = xterm256();
        assert_eq!(
            resolve(Color::Default, [1, 2, 3], false, &palette),
            [1, 2, 3]
        );
        assert_eq!(
            resolve(Color::Rgb(9, 9, 9), [1, 2, 3], false, &palette),
            [9, 9, 9]
        );
    }

    #[test]
    fn bold_brightens_the_low_eight_only() {
        let palette = xterm256();
        assert_eq!(
            resolve(Color::Indexed(1), [0; 3], true, &palette),
            BASE16[9]
        );
        assert_eq!(
            resolve(Color::Indexed(9), [0; 3], true, &palette),
            BASE16[9]
        );
    }

    #[test]
    fn style_runs_group_adjacent_cells() {
        let screen = parse("\x1b[31mred\x1b[0m and more", 20, 1);
        let runs = style_runs(&screen.rows[0]);
        assert_eq!(runs.iter().map(|r| r.1).sum::<usize>(), 20);
        assert_eq!(runs[0].0, 0);
        assert_eq!(runs[0].1, 3);
    }

    #[test]
    fn render_produces_a_frame_of_the_requested_size() {
        let mut r = renderer(320, 180);
        let frame = r.render(&screen_of(&["hello"], 20));
        assert_eq!((frame.width, frame.height), (320, 180));
        assert_eq!(frame.data.len(), 320 * 180 * 3);
    }

    #[test]
    fn render_draws_something_other_than_the_background() {
        let mut r = renderer(320, 180);
        let blank = r.render(&screen_of(&["", "", ""], 20)).data;
        let mut r = renderer(320, 180);
        let filled = r.render(&screen_of(&["hello world", "", ""], 20)).data;
        assert_ne!(blank, filled);
    }

    #[test]
    fn the_grid_fits_inside_the_frame() {
        let mut r = renderer(640, 360);
        r.render(&parse("x", 80, 24));
        let (cw, ch) = r.cell();
        assert!(cw * 80 <= 640, "{cw} * 80 > 640");
        assert!(ch * 24 <= 360, "{ch} * 24 > 360");
    }

    #[test]
    fn the_font_is_reused_while_the_grid_stays_the_same() {
        let mut r = renderer(320, 180);
        r.render(&screen_of(&["a"], 20));
        let size = r.font_size();
        r.render(&screen_of(&["b"], 20));
        assert!((r.font_size() - size).abs() < f32::EPSILON);
    }

    #[test]
    fn a_changed_row_repaints_only_that_row() {
        let mut r = renderer(320, 180);
        r.render(&screen_of(&["one", "two", "three"], 20));
        assert_eq!(r.rows_drawn, 3);
        r.rows_drawn = 0;
        r.render(&screen_of(&["one", "two", "CHANGED"], 20));
        assert_eq!(r.rows_drawn, 1);
    }

    #[test]
    fn an_unchanged_screen_repaints_nothing() {
        let mut r = renderer(320, 180);
        r.render(&screen_of(&["one", "two"], 20));
        r.rows_drawn = 0;
        r.render(&screen_of(&["one", "two"], 20));
        assert_eq!(r.rows_drawn, 0);
    }

    #[test]
    fn a_moved_cursor_repaints_both_rows() {
        let mut r = renderer(320, 180);
        let mut first = screen_of(&["one", "two", "three"], 20);
        first.cursor = Some((0, 0));
        r.render(&first);
        let mut second = screen_of(&["one", "two", "three"], 20);
        second.cursor = Some((0, 2));
        r.rows_drawn = 0;
        r.render(&second);
        assert_eq!(r.rows_drawn, 2);
    }

    #[test]
    fn incremental_output_matches_a_full_repaint() {
        // The whole optimisation is only valid if the pixels come out the same.
        let before = ["\x1b[31mone\x1b[0m", "two", "three"];
        let after = ["\x1b[31mone\x1b[0m", "two", "\x1b[1mdifferent\x1b[0m"];

        let mut incremental = renderer(320, 180);
        incremental.render(&screen_of(&before, 20));
        let stepped = incremental.render(&screen_of(&after, 20)).data;

        let mut fresh = renderer(320, 180);
        let direct = fresh.render(&screen_of(&after, 20)).data;
        assert_eq!(stepped, direct);
    }

    #[test]
    fn a_grid_resize_forces_a_full_repaint() {
        let mut r = renderer(320, 180);
        r.render(&screen_of(&["a", "b"], 20));
        r.rows_drawn = 0;
        r.render(&screen_of(&["a", "b", "c", "d"], 20));
        assert_eq!(r.rows_drawn, 4);
    }

    #[test]
    fn glyphs_are_rasterised_once_per_character() {
        let mut r = renderer(320, 180);
        r.render(&screen_of(&["aaaa bbbb", "aaaa bbbb"], 20));
        assert_eq!(r.glyph_count(), 2); // 'a' and 'b'; spaces are skipped
        r.render(&screen_of(&["bbbb aaaa", "aaaa bbbb"], 20));
        assert_eq!(r.glyph_count(), 2);
    }

    #[test]
    fn a_returned_frame_is_not_disturbed_by_the_next_one() {
        let mut r = renderer(320, 180);
        let first = r.render(&screen_of(&["hello", "world"], 20));
        let snapshot = first.data.clone();
        r.render(&screen_of(&["hello", "CHANGED"], 20));
        assert_eq!(first.data, snapshot);
    }

    #[test]
    fn a_message_card_renders() {
        let mut r = renderer(320, 180);
        let frame = r.message("ttycast", &["no pane to mirror".to_string()]);
        assert_eq!(frame.data.len(), 320 * 180 * 3);
    }
}

#[cfg(test)]
mod glyph_size_probe {
    use super::*;
    use crate::ansi::parse;

    #[test]
    #[ignore = "reports numbers, asserts nothing"]
    fn cell_size_by_column_count() {
        for rows in [44usize, 36, 30, 24, 20, 16] {
            let mut renderer = Renderer::new(
                1280,
                720,
                Theme::default(),
                FontSet::load(None, None).unwrap(),
            );
            renderer.render(&parse("x", 100, rows));
            let (cw, ch) = renderer.cell();
            eprintln!(
                "  {rows:2} Zeilen (100 Spalten) -> Zelle {cw:2}x{ch:2} px, Schrift {:.0}, passt {} Spalten",
                renderer.font_size(),
                1241 / cw
            );
        }
    }
}
