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
    """Draws :class:`~ttycast.ansi.Screen` objects into a fixed-size frame."""

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

    @property
    def font_size(self) -> int:
        return int(getattr(self._regular, "size", 0) or 0)

    def render(self, screen: Screen) -> Image.Image:
        self._prepare(screen.width, screen.height)
        assert self._regular is not None and self._bold is not None
        theme = self.theme
        cw, ch = self._cell
        ox, oy = self._origin

        image = Image.new("RGB", self.size, theme.bg)
        draw = ImageDraw.Draw(image)

        for r, row in enumerate(screen.rows):
            y = oy + r * ch
            for col, text, style in style_runs(row):
                x = ox + col * cw
                bg = resolve(style.bg, theme.bg)
                if bg != theme.bg:
                    draw.rectangle([x, y, x + cw * len(text) - 1, y + ch - 1], fill=bg)
                if text.strip():
                    fg = resolve(style.fg, theme.fg, bold=style.bold)
                    if style.dim:
                        fg = tuple(c // 2 for c in fg)  # type: ignore[assignment]
                    font = self._bold if style.bold else self._regular
                    # Runs are monospace, but a glyph may be proportional after a
                    # font fallback, so each cell is placed on its own column.
                    for i, ch_ in enumerate(text):
                        if ch_ != " ":
                            draw.text((x + i * cw, y), ch_, font=font, fill=fg)
                if style.underline:
                    uy = y + ch - 2
                    fg = resolve(style.fg, theme.fg, bold=style.bold)
                    draw.line([x, uy, x + cw * len(text) - 1, uy], fill=fg)

        if screen.cursor is not None:
            cx, cy = screen.cursor
            if 0 <= cx < screen.width and 0 <= cy < screen.height:
                x, y = ox + cx * cw, oy + cy * ch
                draw.rectangle([x, y, x + cw - 1, y + ch - 1], outline=theme.cursor, width=2)

        return image

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
        return image
