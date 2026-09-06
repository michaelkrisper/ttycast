from ttycast.ansi import Cell, Style, parse
from ttycast.render import BASE16, Renderer, Theme, resolve, style_runs, xterm256


def test_palette_has_all_256_entries():
    palette = xterm256()
    assert len(palette) == 256
    assert palette[:16] == BASE16
    assert palette[196] == (255, 0, 0)


def test_resolve_falls_back_to_the_theme_default():
    assert resolve(None, (1, 2, 3)) == (1, 2, 3)
    assert resolve((9, 9, 9), (1, 2, 3)) == (9, 9, 9)
    assert resolve(300, (1, 2, 3)) == (1, 2, 3)


def test_bold_brightens_the_low_eight_only():
    assert resolve(1, (0, 0, 0), bold=True) == BASE16[9]
    assert resolve(9, (0, 0, 0), bold=True) == BASE16[9]


def test_style_runs_group_adjacent_cells():
    row = [Cell("a"), Cell("b"), Cell("c", Style(fg=2)), Cell("d", Style(fg=2))]
    assert style_runs(row) == [(0, "ab", Style()), (2, "cd", Style(fg=2))]


def test_style_runs_cover_every_column():
    screen = parse("\x1b[31mred\x1b[0m and more", 20, 1)
    runs = style_runs(screen.rows[0])
    assert sum(len(text) for _, text, _ in runs) == 20
    assert runs[0][0] == 0


def test_render_produces_a_frame_of_the_requested_size():
    renderer = Renderer(size=(320, 180))
    frame = renderer.render(parse("hello", 20, 5))
    assert frame.size == (320, 180)
    assert frame.mode == "RGB"


def test_render_draws_something_other_than_the_background():
    theme = Theme()
    renderer = Renderer(size=(320, 180), theme=theme)
    blank = renderer.render(parse("", 20, 5))
    filled = renderer.render(parse("hello world", 20, 5))
    assert blank.tobytes() != filled.tobytes()


def test_font_is_reused_while_the_grid_stays_the_same():
    renderer = Renderer(size=(320, 180))
    renderer.render(parse("a", 20, 5))
    size = renderer.font_size
    renderer.render(parse("b", 20, 5))
    assert renderer.font_size == size
    renderer.render(parse("c", 60, 20))
    assert renderer.font_size < size


def test_grid_fits_inside_the_frame():
    renderer = Renderer(size=(640, 360))
    renderer.render(parse("x", 80, 24))
    cell_w, cell_h = renderer._cell
    assert cell_w * 80 <= 640
    assert cell_h * 24 <= 360


def test_message_card_renders():
    renderer = Renderer(size=(320, 180))
    frame = renderer.message("ttycast", ["line one", "line two"])
    assert frame.size == (320, 180)


def screen_of(lines, width=20):
    return parse("\n".join(lines), width, len(lines))


def test_a_changed_row_repaints_only_that_row():
    renderer = Renderer(size=(320, 180))
    before = screen_of([f"line {i}" for i in range(8)])
    renderer.render(before)
    assert renderer.rows_drawn == 8  # first frame paints everything

    after = screen_of([f"line {i}" for i in range(7)] + ["changed"])
    renderer.rows_drawn = 0
    renderer.render(after)
    assert renderer.rows_drawn == 1


def test_an_unchanged_screen_repaints_nothing():
    renderer = Renderer(size=(320, 180))
    screen = screen_of([f"line {i}" for i in range(8)])
    renderer.render(screen)
    renderer.rows_drawn = 0
    renderer.render(screen_of([f"line {i}" for i in range(8)]))
    assert renderer.rows_drawn == 0


def test_a_moved_cursor_repaints_both_rows():
    renderer = Renderer(size=(320, 180))
    screen = screen_of([f"line {i}" for i in range(8)])
    screen.cursor = (0, 1)
    renderer.render(screen)

    moved = screen_of([f"line {i}" for i in range(8)])
    moved.cursor = (0, 5)
    renderer.rows_drawn = 0
    renderer.render(moved)
    assert renderer.rows_drawn == 2  # the row it left and the row it entered


def test_incremental_output_matches_a_full_repaint():
    """The whole optimisation is only valid if the pixels come out the same."""
    lines = [f"\x1b[3{i % 8}mline {i}\x1b[0m" for i in range(8)]
    changed = lines[:7] + ["\x1b[1mdifferent now\x1b[0m"]

    incremental = Renderer(size=(320, 180))
    incremental.render(screen_of(lines))
    step = incremental.render(screen_of(changed))

    fresh = Renderer(size=(320, 180))
    assert fresh.render(screen_of(changed)).tobytes() == step.tobytes()


def test_a_grid_resize_forces_a_full_repaint():
    renderer = Renderer(size=(320, 180))
    renderer.render(screen_of(["a", "b"]))
    renderer.rows_drawn = 0
    renderer.render(screen_of(["a", "b", "c", "d"]))
    assert renderer.rows_drawn == 4


def test_glyphs_are_rasterised_once_per_character():
    renderer = Renderer(size=(320, 180))
    renderer.render(screen_of(["aaaa bbbb", "aaaa bbbb"]))
    cached = len(renderer._glyphs)
    renderer.render(screen_of(["bbbb aaaa", "aaaa bbbb"]))
    assert cached == len(renderer._glyphs) == 2  # 'a' and 'b'; spaces are skipped


def test_the_returned_frame_is_not_the_live_canvas():
    renderer = Renderer(size=(320, 180))
    first = renderer.render(screen_of(["hello", "world"]))
    snapshot = first.tobytes()
    renderer.render(screen_of(["hello", "CHANGED"]))
    assert first.tobytes() == snapshot  # the earlier frame was not drawn over
