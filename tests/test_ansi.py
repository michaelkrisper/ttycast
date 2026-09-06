from ttycast.ansi import DEFAULT_STYLE, Style, apply_sgr, parse


def test_plain_text_is_padded_to_the_grid():
    screen = parse("hi", 5, 2)
    assert screen.width == 5
    assert "".join(c.char for c in screen.rows[0]) == "hi   "
    assert "".join(c.char for c in screen.rows[1]) == "     "


def test_long_lines_are_cut_to_the_grid():
    screen = parse("abcdefgh", 3, 1)
    assert "".join(c.char for c in screen.rows[0]) == "abc"


def test_basic_colors():
    screen = parse("\x1b[31mred\x1b[0mplain", 8, 1)
    assert screen.rows[0][0].style.fg == 1
    assert screen.rows[0][3].style == DEFAULT_STYLE


def test_bright_colors_map_to_the_upper_eight():
    screen = parse("\x1b[91mx", 1, 1)
    assert screen.rows[0][0].style.fg == 9


def test_256_and_truecolor():
    screen = parse("\x1b[38;5;196ma\x1b[48;2;10;20;30mb", 2, 1)
    assert screen.rows[0][0].style.fg == 196
    assert screen.rows[0][1].style.bg == (10, 20, 30)


def test_subparameter_form_is_accepted():
    screen = parse("\x1b[38:2:1:2:3mx", 1, 1)
    assert screen.rows[0][0].style.fg == (1, 2, 3)


def test_attributes_toggle_independently():
    style = apply_sgr(DEFAULT_STYLE, [1, 4, 7])
    assert (style.bold, style.underline, style.reverse) == (True, True, True)
    style = apply_sgr(style, [22])
    assert style.bold is False
    assert style.underline is True


def test_reset_clears_everything():
    style = apply_sgr(Style(fg=3, bold=True), [0])
    assert style == DEFAULT_STYLE


def test_reverse_is_baked_into_colors():
    style = Style(fg=1, bg=2, reverse=True).resolved()
    assert (style.fg, style.bg, style.reverse) == (2, 1, False)


def test_non_sgr_sequences_are_dropped():
    screen = parse("a\x1b[2Kb", 3, 1)
    assert "".join(c.char for c in screen.rows[0]) == "ab "


def test_osc_sequences_are_dropped():
    screen = parse("\x1b]0;window title\x07ok", 2, 1)
    assert "".join(c.char for c in screen.rows[0]) == "ok"


def test_style_survives_a_line_break():
    screen = parse("\x1b[32ma\nb", 1, 2)
    assert screen.rows[1][0].style.fg == 2


def test_tabs_expand_to_the_next_stop():
    screen = parse("a\tb", 10, 1)
    assert "".join(c.char for c in screen.rows[0]) == "a       b "


def test_key_changes_only_when_content_changes():
    assert parse("a", 2, 1).key() == parse("a", 2, 1).key()
    assert parse("a", 2, 1).key() != parse("b", 2, 1).key()
    assert parse("a", 2, 1).key() != parse("\x1b[31ma", 2, 1).key()
