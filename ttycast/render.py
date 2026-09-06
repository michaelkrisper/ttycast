"""Turn a cell grid into an RGB frame sized for a TV.

The grid is drawn as style runs rather than per cell: a terminal line rarely has
more than a handful of colour changes, so a 80x24 screen costs ~100 draw calls
instead of ~2000, which is what keeps a pure-Pillow renderer viable at 10 fps.
"""

from __future__ import annotations

import functools
import shutil
import subprocess
from dataclasses import dataclass

from PIL import Image, ImageDraw, ImageFont

from ttycast.ansi import Cell, Screen, Style

RGB = tuple[int, int, int]

#: xterm's first 16 entries, the ones a terminal theme actually swaps out.
BASE16: tuple[RGB, ...] = (
    (0, 0, 0),
    (205, 49, 49),
    (13, 188, 121),
    (229, 229, 16),
    (36, 114, 200),
    (188, 63, 188),
    (17, 168, 205),
    (229, 229, 229),
    (102, 102, 102),
    (241, 76, 76),
    (35, 209, 139),
    (245, 245, 67),
    (59, 142, 234),
    (214, 112, 214),
    (41, 184, 219),
    (255, 255, 255),
)


@functools.lru_cache(maxsize=1)
def xterm256() -> tuple[RGB, ...]:
    colors = list(BASE16)
    levels = (0, 95, 135, 175, 215, 255)
    for r in levels:
        for g in levels:
            for b in levels:
                colors.append((r, g, b))
    colors.extend((v, v, v) for v in range(8, 239, 10))
    return tuple(colors)


@dataclass(frozen=True, slots=True)
class Theme:
    fg: RGB = (222, 222, 222)
    bg: RGB = (12, 12, 14)
    cursor: RGB = (255, 176, 0)
    #: TVs overscan; a margin keeps the outermost cells on the panel.
    margin: float = 0.03


def resolve(color, theme_default: RGB, bold: bool = False) -> RGB:
    if color is None:
        return theme_default
    if isinstance(color, tuple):
        return color
    if bold and color < 8:
        color += 8
    palette = xterm256()
    return palette[color] if 0 <= color < len(palette) else theme_default


@functools.lru_cache(maxsize=8)
def find_font(pattern: str) -> str | None:
    """Ask fontconfig for a font file, so we do not have to ship one."""
    if shutil.which("fc-match") is None:
        return None
    try:
        out = subprocess.run(  # noqa: S603
            ["fc-match", "-f", "%{file}", pattern], capture_output=True, text=True, timeout=5
        )
    except (OSError, subprocess.SubprocessError):
        return None
    path = out.stdout.strip()
    return path or None


AnyFont = ImageFont.FreeTypeFont | ImageFont.ImageFont


def load_font(path: str | None, size: int) -> AnyFont:
    if path:
        return ImageFont.truetype(path, size)
    return ImageFont.load_default(size)


def cell_metrics(font: AnyFont) -> tuple[int, int]:
    """Advance width and line height of a monospace font at its current size."""
    width = max(1, round(font.getlength("M")))
    ascent, descent = font.getmetrics()
    return width, max(1, ascent + descent)


def fit_font_size(path: str | None, cols: int, rows: int, box: tuple[int, int]) -> int:
    """Largest size whose ``cols`` x ``rows`` grid still fits inside ``box``."""
    max_w, max_h = box
    lo, hi, best = 4, 200, 4
    while lo <= hi:
        mid = (lo + hi) // 2
        cw, ch = cell_metrics(load_font(path, mid))
        if cw * cols <= max_w and ch * rows <= max_h:
            best, lo = mid, mid + 1
        else:
            hi = mid - 1
    return best


def style_runs(row: list[Cell]) -> list[tuple[int, str, Style]]:
    """Group a row into ``(start_column, text, style)`` runs."""
    runs: list[tuple[int, str, Style]] = []
    start = 0
    chars: list[str] = []
    current: Style | None = None
    for col, cell in enumerate(row):
        style = cell.style.resolved()
        if current is None:
            current, start = style, col
        elif style != current:
            runs.append((start, "".join(chars), current))
            chars, current, start = [], style, col
        chars.append(cell.char)
    if current is not None:
        runs.append((start, "".join(chars), current))
    return runs


class Renderer:
    """Draws :class:`~ttycast.ansi.Screen` objects into a fixed-size frame.

    Two things keep this cheap enough to run at 10 fps on an old laptop:

    * a whole style run is drawn with **one** ``draw.text`` call instead of one
      per character, which moves the per-glyph loop from Python into FreeType;
    * consecutive frames are diffed by row, so typing a character repaints one
      row rather than the screen.

    The canvas therefore persists between calls, and :meth:`render` hands out a
    copy so a consumer encoding the previous frame never sees a half-drawn one.
    """

    def __init__(
        self,
        size: tuple[int, int] = (1280, 720),
        theme: Theme | None = None,
        font: str | None = None,
        bold_font: str | None = None,
    ) -> None:
        self.size = size
        self.theme = theme or Theme()
        self.font_path = font or find_font("monospace")
        self.bold_path = bold_font or find_font("monospace:bold") or self.font_path
        self._grid: tuple[int, int] | None = None
        self._regular: AnyFont | None = None
        self._bold: AnyFont | None = None
        self._cell: tuple[int, int] = (1, 1)
        self._origin: tuple[int, int] = (0, 0)
        self._canvas: Image.Image | None = None
        self._draw: ImageDraw.ImageDraw | None = None
        self._prev_rows: list[list[Cell]] | None = None
        self._prev_cursor: tuple[int, int] | None = None
        # Rasterising a glyph costs ~2.5 ms in FreeType, and a terminal draws
        # the same few hundred glyphs over and over, so each one is rendered
        # once into an alpha mask and then blitted.
        self._glyphs: dict[tuple[str, bool], Image.Image] = {}
        self._fill: RGB = self.theme.fg
        self.full_redraws = 0
        self.rows_drawn = 0

    def _prepare(self, cols: int, rows: int) -> None:
        if self._grid == (cols, rows):
            return
        w, h = self.size
        inset = self.theme.margin
        box = (int(w * (1 - 2 * inset)), int(h * (1 - 2 * inset)))
        size = fit_font_size(self.font_path, cols, rows, box)
        self._regular = load_font(self.font_path, size)
        self._bold = load_font(self.bold_path, size)
        self._cell = cell_metrics(self._regular)
        cw, ch = self._cell
        self._origin = ((w - cw * cols) // 2, (h - ch * rows) // 2)
        self._grid = (cols, rows)
        self._glyphs.clear()
        self._canvas = None  # geometry moved, nothing on the old canvas is valid

    @property
    def font_size(self) -> int:
        return int(getattr(self._regular, "size", 0) or 0)

    def _glyph(self, char: str, bold: bool) -> Image.Image:
        """The alpha mask for one character, rasterised at most once."""
        key = (char, bold)
        mask = self._glyphs.get(key)
        if mask is not None:
            return mask
        font = self._bold if bold else self._regular
        assert font is not None
        cw, ch = self._cell
        # A double-width character (CJK, some emoji) is allowed to spill into
        # the cell tmux left blank next to it, the way a terminal draws it.
        width = max(cw, min(2 * cw, round(font.getlength(char))))
        mask = Image.new("L", (width, ch), 0)
        ImageDraw.Draw(mask).text((0, 0), char, font=font, fill=255)
        self._glyphs[key] = mask
        return mask

    def _draw_row(self, row: list[Cell], index: int) -> None:
        draw, theme = self._draw, self.theme
        assert draw is not None and self._regular is not None and self._bold is not None
        cw, ch = self._cell
        ox, oy = self._origin
        y = oy + index * ch
        width = self.size[0]

        draw.rectangle([0, y, width - 1, y + ch - 1], fill=theme.bg)

        for col, text, style in style_runs(row):
            x = ox + col * cw
            span = cw * len(text)
            bg = resolve(style.bg, theme.bg)
            if bg != theme.bg:
                draw.rectangle([x, y, x + span - 1, y + ch - 1], fill=bg)
            if not text.strip():
                continue
            fg = resolve(style.fg, theme.fg, bold=style.bold)
            if style.dim:
                fg = (fg[0] // 2, fg[1] // 2, fg[2] // 2)
            self._fill = fg
            self._blit_run(text, style.bold, x, y, span)
            if style.underline:
                uy = y + ch - 2
                draw.line([x, uy, x + span - 1, uy], fill=fg)
        self.rows_drawn += 1

    def _blit_run(self, text: str, bold: bool, x: int, y: int, span: int) -> None:
        """Compose a run's glyph masks, then lay the colour down in one paste."""
        canvas = self._canvas
        assert canvas is not None
        cw, ch = self._cell
        # One cell of slack so a double-width glyph at the end is not clipped.
        mask = Image.new("L", (span + cw, ch), 0)
        for i, char in enumerate(text):
            if char != " ":
                mask.paste(self._glyph(char, bold), (i * cw, 0))
        right = min(x + span + cw, self.size[0])
        if right <= x:
            return
        if right < x + span + cw:
            mask = mask.crop((0, 0, right - x, ch))
        canvas.paste(self._fill, (x, y, right, y + ch), mask)

    def _draw_cursor(self, cursor: tuple[int, int], screen: Screen) -> None:
        cx, cy = cursor
        if not (0 <= cx < screen.width and 0 <= cy < screen.height):
            return
        assert self._draw is not None
        cw, ch = self._cell
        ox, oy = self._origin
        x, y = ox + cx * cw, oy + cy * ch
        self._draw.rectangle([x, y, x + cw - 1, y + ch - 1], outline=self.theme.cursor, width=2)

    def _dirty_rows(self, screen: Screen) -> set[int]:
        """Rows that differ from the last frame, plus the two the cursor touches."""
        previous = self._prev_rows
        assert previous is not None
        dirty = {i for i, row in enumerate(screen.rows) if row != previous[i]}
        for cursor in (self._prev_cursor, screen.cursor):
            if cursor is not None and 0 <= cursor[1] < screen.height:
                dirty.add(cursor[1])
        return dirty

    def render(self, screen: Screen) -> Image.Image:
        self._prepare(screen.width, screen.height)

        reusable = (
            self._canvas is not None
            and self._prev_rows is not None
            and len(self._prev_rows) == screen.height
        )
        if reusable:
            rows = self._dirty_rows(screen)
        else:
            self._canvas = Image.new("RGB", self.size, self.theme.bg)
            self._draw = ImageDraw.Draw(self._canvas)
            rows = set(range(screen.height))
            self.full_redraws += 1

        for index in sorted(rows):
            self._draw_row(screen.rows[index], index)

        if screen.cursor is not None:
            self._draw_cursor(screen.cursor, screen)

        # Rows are frozen cells from a freshly parsed screen; no copy needed.
        self._prev_rows = screen.rows
        self._prev_cursor = screen.cursor
        assert self._canvas is not None
        # The canvas keeps being drawn on, so hand out a snapshot.
        return self._canvas.copy()

    def message(self, title: str, lines: list[str]) -> Image.Image:
        """A standalone card, used for "waiting for a pane" style states."""
        image = Image.new("RGB", self.size, self.theme.bg)
        draw = ImageDraw.Draw(image)
        w, h = self.size
        big = load_font(self.bold_path, max(16, h // 14))
        small = load_font(self.font_path, max(12, h // 26))
        draw.text((w // 2, h // 2 - h // 10), title, font=big, fill=self.theme.cursor, anchor="mm")
        for i, line in enumerate(lines):
            draw.text(
                (w // 2, h // 2 + i * (h // 18)), line, font=small, fill=self.theme.fg, anchor="mm"
            )
        # A card is not part of the incremental stream; drop the diff state so
        # the next real frame repaints everything.
        self._canvas = None
        self._prev_rows = None
        return image
