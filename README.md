# ttycast

Put your terminal on the TV, from the terminal.

`ttycast` mirrors a tmux pane to a television over the network. It reads the
pane as text, draws it at TV resolution with a big monospace font, and hands the
result to whatever the TV speaks: a plain browser URL, DLNA, or Miracast.

The idea is a laptop in the corner of the room, a Bluetooth keyboard on the
sofa, and the shell on the big screen.

```
$ ttycast
ttycast 0.1.0  1280x720 @ 5 fps  backend=browser
  open on the TV:  http://192.168.1.42:8009/
  raw stream:      http://192.168.1.42:8009/stream.mjpg
  14 frames  http://192.168.1.42:8009/  (ctrl-c to stop)
```

The server starts with the command and dies with it. No daemon, no config file,
no state on disk.

## Why it can be this cheap

A terminal is not a video signal. It is a grid of characters that changes a few
times a second, and only in places. So `ttycast` never captures the screen:

- **Capture** is `tmux capture-pane`, a few kilobytes of text and colour codes,
  fetched together with the pane geometry in a single tmux invocation.
- **An unchanged pane costs one string compare.** The raw capture is diffed
  before it is parsed, so an idle shell never reaches the renderer at all.
- **A changed pane repaints the rows that changed**, not the screen. Typing a
  character redraws one row.
- **Glyphs are rasterised once** and then blitted. FreeType is the expensive
  part of drawing text, and a terminal uses the same hundred-odd glyphs forever.
- Because the source is text, the output is **crisp at any resolution**. No
  upscaling of a laptop panel, no unreadable 8pt fonts on a screen three metres
  away.

## Performance

Measured on an Intel i5-4258U (a 2013 dual-core laptop), 1280x720 output, a
149x44 pane:

| Situation | Cost per frame | Ceiling |
|---|---|---|
| Idle - nothing on screen moved | 3.7 ms (one tmux call, no render) | - |
| Typing - one row changed | ~5 ms | ~200 fps |
| Full-screen scrolling - every row changed | ~29 ms | **34 fps sustained** |
| H.264 encode, 720p, libopenh264 | 9 ms | 111 fps |

The first version of the renderer drew each character with its own `draw.text`
call and took **509 ms per frame**, which capped the whole thing at 2 fps. Run
drawing, the glyph cache and row diffing took that to 15 ms. If you change the
renderer, `tests/test_render.py` asserts that the incremental output is
byte-identical to a full repaint, and that a one-row change repaints one row.

Latency, end to end, is dominated by the transport and not by ttycast:
`browser` is one frame, `miracast` is sub-second, and `dlna` is seconds because
the television buffers.

## Install

```bash
pipx install ttycast          # or: pip install ttycast
```

Needs Python 3.10+, `tmux`, and a monospace font. `ffmpeg` with `libx264` is
only needed for the DLNA and Miracast backends.

Start by asking what works on your machine:

```bash
ttycast doctor
```

## Backends

| Backend | Reaches the TV by | Latency | Needs |
|---|---|---|---|
| `browser` | you open a URL | one frame | nothing |
| `dlna` | pushing a URL to the TV over UPnP | seconds | ffmpeg, TV with content sharing on |
| `miracast` | a wireless display session | sub-second | ffmpeg, **Wi-Fi Direct capable adapter** |
| `preview` | writing a PNG | n/a | nothing |

### browser

```bash
ttycast                       # the default
ttycast --port 8080
```

Serves MJPEG at `http://<your-ip>:8009/`. Open it in the TV's browser, on a
phone, or on another laptop. This is the one that always works.

### dlna

```bash
ttycast discover              # what is on the network?
ttycast --backend dlna
ttycast --backend dlna --renderer "Living Room"
```

`ttycast` finds a DLNA MediaRenderer, hands it a URL into its own HTTP server,
and the TV pulls the H.264 stream itself. Only an address travels; the TV does
the fetching.

**If discovery finds nothing, it is almost always the TV.** Most sets only
announce a renderer while *content sharing* / *screen sharing* is enabled in
their settings, and never while they are off.

Expect one to several seconds of latency. Renderers buffer, and a sender cannot
switch that off. Fine for watching a build or a log, awkward for typing.

### miracast

```bash
ttycast --backend miracast --sink-host 192.168.49.1 --sink-port 5000
```

Miracast is the only backend that mirrors rather than plays a file, so it is the
only one fast enough to actually work at the TV.

It is also the one with a hardware requirement that cannot be worked around in
software. Miracast rides on Wi-Fi Direct, and Wi-Fi Direct needs a driver that
offers `P2P-GO`/`P2P-client` interface modes. Plenty of laptops do not have one
— anything on Broadcom's proprietary `wl` driver, for instance. Check with:

```bash
iw list | grep -A10 "Supported interface modes"
```

If `P2P-GO` is missing, no software can add it; a USB adapter on `mt76`
(e.g. MT7612U), `rtw88`, or an Intel AX2xx will.

**Status: experimental.** The RTSP capability negotiation
([`ttycast/wfd.py`](ttycast/wfd.py)) and the RTP transport are implemented and
unit-tested, and `ttycast` will send to a sink you have already associated with
`--sink-host`. Automatic session setup — driving wpa_supplicant through the
Wi-Fi Direct connect and the M1..M7 exchange — is not wired up yet. The path is
unverified end to end because no P2P-capable radio was available while it was
written. Reports from real sinks are very welcome.

## Options worth knowing

```
-t, --target auto     which pane to mirror
-f, --fps 10          capture rate; 1 is fine, 30 is smooth
-s, --size 1280x720   output resolution
    --bitrate 2M      H.264 bitrate for dlna/miracast
    --font PATH       a specific monospace font
    --once            render one frame and exit
```

`--target` decides what ends up on screen:

- `auto` (default) follows the **active pane of your current tmux session**, so
  switching windows switches what the TV shows. `ttycast`'s own pane is skipped,
  so running it in a spare window does not mirror its own output.
- `self` pins the pane `ttycast` runs in.
- `new` creates a detached `ttycast` session; attach to it with
  `tmux attach -t ttycast` and everything in it goes to the TV.
- Anything else is passed to tmux as a target, e.g. `work:2.0`.

If you are not inside tmux at all, `ttycast` creates the `ttycast` session for
you.

## How it fits together

```
tmux capture-pane -e          text + SGR
        |
   ttycast/ansi.py            -> cell grid
        |
   ttycast/render.py          -> RGB frame (Pillow, style runs)
        |
   ttycast/framebus.py        latest frame, consumers wake on change
        |
        +--> server.py        /stream.mjpg   -> browser
        +--> encoder.py       H.264/MPEG-TS  -> /live.ts -> DLNA renderer
        +--> encoder.py       H.264/RTP      -> Miracast sink
```

Backends do not touch capture or rendering. They only decide how the display
finds out about the stream: you tell it (`browser`), UPnP tells it (`dlna`), or
a wireless display session carries it (`miracast`).

## Security

`ttycast` serves **your terminal, unauthenticated, to the whole LAN**. That is
the point — a TV cannot log in — but it means anything on the network can watch
you type. Do not run it on a network you do not trust, and remember that a
terminal shows secrets: keys echoed into a shell, tokens in a log tail, the
contents of whatever file you just opened.

There is no authentication and no TLS, and adding them would not help: no TV
would use them.

## Development

```bash
git clone https://github.com/michaelkrisper/ttycast
cd ttycast
python -m venv .venv && .venv/bin/pip install -e '.[dev]'
.venv/bin/pytest
.venv/bin/ruff check .
```

The protocol layers are pure functions with no I/O — `ansi`, `wfd`, `upnp`,
`wifi` — so they are tested without a network, a TV, or a radio.

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
