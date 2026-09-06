"""Parse the SGR-decorated text that ``tmux capture-pane -e`` emits into a cell grid.

Only SGR (``ESC [ ... m``) is interpreted; every other escape sequence is skipped,
because ``capture-pane`` renders a settled screen and does not emit cursor motion.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field, replace

#: ``None`` means "terminal default", an ``int`` is an xterm palette index
#: (0-255) and a 3-tuple is a truecolor RGB value.
Color = None | int | tuple[int, int, int]

_CSI = re.compile(r"\x1b\[([0-9;:]*)([a-zA-Z])")
_OSC = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")


@dataclass(frozen=True, slots=True)
class Style:
    fg: Color = None
    bg: Color = None
    bold: bool = False
    dim: bool = False
    italic: bool = False
    underline: bool = False
    reverse: bool = False

    def resolved(self) -> Style:
        """Bake ``reverse`` into the colors so renderers do not have to care."""
        if not self.reverse:
            return self
        return replace(self, fg=self.bg, bg=self.fg, reverse=False)


DEFAULT_STYLE = Style()


@dataclass(frozen=True, slots=True)
class Cell:
    char: str = " "
    style: Style = DEFAULT_STYLE


@dataclass(slots=True)
class Screen:
    width: int
    height: int
    rows: list[list[Cell]] = field(default_factory=list)
    cursor: tuple[int, int] | None = None

    def key(self) -> tuple:
        """Cheap identity used to detect "nothing changed, skip the re-render"."""
        return (self.width, self.height, self.cursor, tuple(tuple(r) for r in self.rows))


def _take(params: list[int], i: int) -> tuple[Color, int]:
    """Read an extended color starting at ``params[i]`` (which is 5 or 2)."""
    if i < len(params) and params[i] == 5:
        return (params[i + 1] if i + 1 < len(params) else 0), i + 2
    if i < len(params) and params[i] == 2:
        rgb = tuple((params[i + 1 + k] if i + 1 + k < len(params) else 0) for k in range(3))
        return rgb, i + 4  # type: ignore[return-value]
    return None, i + 1


def apply_sgr(style: Style, params: list[int]) -> Style:
    """Fold one SGR parameter list into ``style``."""
    if not params:
        params = [0]
    i = 0
    while i < len(params):
        p = params[i]
        i += 1
        if p == 0:
            style = DEFAULT_STYLE
        elif p == 1:
            style = replace(style, bold=True)
        elif p == 2:
            style = replace(style, dim=True)
        elif p == 3:
            style = replace(style, italic=True)
        elif p == 4:
            style = replace(style, underline=True)
        elif p == 7:
            style = replace(style, reverse=True)
        elif p == 22:
            style = replace(style, bold=False, dim=False)
        elif p == 23:
            style = replace(style, italic=False)
        elif p == 24:
            style = replace(style, underline=False)
        elif p == 27:
            style = replace(style, reverse=False)
        elif 30 <= p <= 37:
            style = replace(style, fg=p - 30)
        elif p == 38:
            color, i = _take(params, i)
            style = replace(style, fg=color)
        elif p == 39:
            style = replace(style, fg=None)
        elif 40 <= p <= 47:
            style = replace(style, bg=p - 40)
        elif p == 48:
            color, i = _take(params, i)
            style = replace(style, bg=color)
        elif p == 49:
            style = replace(style, bg=None)
        elif 90 <= p <= 97:
            style = replace(style, fg=p - 90 + 8)
        elif 100 <= p <= 107:
            style = replace(style, bg=p - 100 + 8)
    return style


def _params(raw: str) -> list[int]:
    # Sub-parameters (``38:2:r:g:b``) are flattened; that is close enough for SGR.
    out = []
    for part in raw.replace(":", ";").split(";"):
        out.append(int(part) if part.isdigit() else 0)
    return out


def parse(text: str, width: int, height: int) -> Screen:
    """Turn captured pane text into a fixed ``width`` x ``height`` cell grid."""
    text = _OSC.sub("", text)
    rows: list[list[Cell]] = []
    style = DEFAULT_STYLE

    for line in text.split("\n")[:height]:
        row: list[Cell] = []
        pos = 0
        while pos < len(line):
            m = _CSI.search(line, pos)
            if m is None:
                chunk, pos = line[pos:], len(line)
            else:
                chunk, pos = line[pos : m.start()], m.end()
            for ch in chunk:
                if ch == "\r" or ch == "\x1b":
                    continue
                if ch == "\t":
                    row.extend([Cell(" ", style)] * (8 - len(row) % 8))
                    continue
                row.append(Cell(ch, style))
            if m is not None and m.group(2) == "m":
                style = apply_sgr(style, _params(m.group(1)))
        row = row[:width]
        row.extend([Cell(" ", style)] * (width - len(row)))
        rows.append(row)

    blank = [Cell(" ", DEFAULT_STYLE)] * width
    while len(rows) < height:
        rows.append(list(blank))
    return Screen(width=width, height=height, rows=rows)
