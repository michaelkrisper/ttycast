# ttycast

Put your terminal on the TV, from the terminal.

`ttycast` mirrors a tmux pane to a television over the network. It reads the
pane as text, draws it at TV resolution with a big monospace font, and hands the
result to whatever the TV speaks: a plain browser URL, DLNA, or Miracast.

The idea is a laptop in the corner of the room, a Bluetooth keyboard on the
sofa, and the shell on the big screen.

```
$ ttycast
ttycast 0.2.0  1280x720 @ 10 fps  backend=browser
  open on the TV:  http://192.168.1.42:8009/
  raw stream:      http://192.168.1.42:8009/stream.mjpg
  14 frames   1.2 ms/frame  http://192.168.1.42:8009/  (ctrl-c to stop)
```

One static binary, ~2 MB. The server starts with the command and dies with it.
No daemon, no config file, no state on disk.

## Why it can be this cheap

A terminal is not a video signal. It is a grid of characters that changes a few
times a second, and only in places. So `ttycast` never captures the screen:

- **Capture** is `tmux capture-pane`, a few kilobytes of text and colour codes,
  fetched together with the pane geometry in a single tmux invocation.
- **An unchanged pane costs one string compare.** The raw capture is diffed
  before it is parsed, so an idle shell never reaches the renderer at all.
- **A changed pane repaints the rows that changed**, not the screen. Typing a
  character redraws one row.
- **Glyphs are rasterised once** into coverage masks and then blitted.
- Because the source is text, the output is **crisp at any resolution**. No
  upscaling of a laptop panel, no unreadable 8pt fonts on a screen three metres
  away.

## Performance

Measured on an Intel i5-4258U (a 2013 dual-core laptop), 1280x720 output, a
149x44 pane. The `0.1` column is the Python implementation this replaced,
measured the same way on the same machine:

| | 0.1 (Python) | 0.2 (Rust) |
|---|---|---|
| Full-screen scrolling, end to end | 34 fps | **121 fps** |
| Render, every row changed | 14.3 ms | **1.4 ms** |
| Render, one row changed (typing) | 5 ms | **0.3 ms** |
| ANSI parse | 2.2 ms | **0.03 ms** |
| JPEG encode | 4.0 ms | **3.3 ms** |
| Process startup | 185 ms | **1.3 ms** |

Two of those numbers are worth explaining, because they were not free:

- **JPEG.** The pure-Rust encoders are about three times slower than
  libjpeg-turbo, so this uses mozjpeg. mozjpeg optimises for file size by
  default - trellis quantisation, optimised Huffman tables - which costs 90 ms
  per 720p frame; `set_fastest_defaults()` and 4:2:0 chroma bring it to 3.3 ms.
  A live terminal wants the milliseconds far more than the last few kilobytes.
- **Capture.** About 3.7 ms of every frame is spawning tmux and reading its
  pipe. That is process overhead and no language changes it, which is why
  capture was folded into a single invocation rather than three.

Latency, end to end, is dominated by the transport and not by ttycast:
`browser` is one frame, `miracast` is sub-second, and `dlna` is seconds because
the television buffers.

## Install

```bash
cargo install --git https://github.com/michaelkrisper/ttycast
```

Building needs `nasm` (mozjpeg compiles libjpeg-turbo's SIMD kernels).
Running needs `tmux` and a monospace font. `ffmpeg` is only needed for the DLNA
and Miracast backends; ttycast picks whichever H.264 encoder it finds
(`libx264`, `libopenh264` or `h264_vaapi`), so a distribution shipping a
patent-free ffmpeg works as it is.

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
([`src/wfd.rs`](src/wfd.rs)) and the RTP transport are implemented and
unit-tested, and `ttycast` will send to a sink you have already associated with
`--sink-host`. Automatic session setup — driving wpa_supplicant through the
Wi-Fi Direct connect and the M1..M7 exchange — is not wired up yet. The path is
unverified end to end because no P2P-capable radio was available while it was
written. Reports from real sinks are very welcome.

## Turning the laptop's own screen off

The laptop in the corner does not need its own panel lit while the television is
showing the same thing.

```bash
ttycast --screen-off                  # pick the best method available
ttycast --screen-off backlight        # force one: sway, wlopm, xset, backlight
ttycast screen-on                     # escape hatch, see below
```

The screen goes dark as soon as something connects to the stream and comes back
five seconds after the last viewer leaves. The grace period is there so an MJPEG
client reconnecting does not flap the panel on and off.

Measured on the same i5-4258U, at full brightness:

| State | Package power | Saved |
|---|---|---|
| Screen on | 23.4 W | - |
| Backlight at zero | 17.2 W | 6.2 W |
| Output powered down (`sway`) | **13.6 W** | **9.8 W** |

Asking the compositor to power the output down beats dimming by 3.6 W, because
the panel electronics stop as well and nothing is composited for a disabled
output — which also gives some CPU back. That is why ttycast prefers it, and
falls back to the backlight only where no compositor offers output power
management.

**If ttycast is killed outright** it never gets to restore anything, and you are
left looking at a black laptop. `ttycast screen-on` undoes every method; so does
`swaymsg output '*' dpms on` or `brightnessctl -r`, both of which can be typed
blind.

## Options worth knowing

```
-t, --target auto     which pane to mirror
-f, --fps 10          capture rate; 1 is fine, 60 is smooth
-s, --size 1280x720   output resolution
    --bitrate 2M      H.264 bitrate for dlna/miracast
    --encoder NAME    force an ffmpeg H.264 encoder
    --font PATH       a specific monospace font
    --screen-off      darken this laptop's screen while someone is watching
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
   src/ansi.rs                -> cell grid
        |
   src/render.rs              -> RGB frame (glyph cache, row diffing)
        |
   src/framebus.rs            latest frame, consumers wake on change
        |
        +--> server.rs        /stream.mjpg   -> browser
        +--> encoder.rs       H.264/MPEG-TS  -> /live.ts -> DLNA renderer
        +--> encoder.rs       H.264/RTP      -> Miracast sink
```

Backends do not touch capture or rendering. They only decide how the display
finds out about the stream: you tell it (`browser`), UPnP tells it (`dlna`), or
a wireless display session carries it (`miracast`).

Five dependencies: `clap`, `fontdue`, `mozjpeg`, `png` and `libc`. The HTTP
server, the SSDP/SOAP control point and the RTSP message layer are hand-rolled
on `std::net` — the whole HTTP surface is five routes, and a framework would
cost more than it saves.

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
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The protocol layers are pure functions with no I/O — `ansi`, `wfd`, `upnp`,
`wifi` — so they are tested without a network, a TV, or a radio. `render` is
tested against the property that matters: incremental output must be
byte-identical to a full repaint.

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
