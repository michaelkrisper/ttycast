# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Rendering is roughly 30x faster: style runs are drawn in one call, glyphs are
  rasterised once into cached masks, and consecutive frames are diffed by row.
  A full repaint went from 509 ms to 15 ms at 720p on a 2013 dual-core laptop,
  which lifts full-screen scrolling from 2 fps to 34 fps.
- Capture asks tmux for geometry, cursor and contents in a single invocation
  instead of three, and resolves the tmux binary once instead of walking `PATH`
  on every frame.
- An unchanged pane is now detected by comparing the raw capture, so idle ticks
  skip both parsing and rendering.
- The default frame rate is 10 fps, up from 5.
- The status line reports milliseconds per frame.

## [0.1.0] - 2026-09-06

First release.

### Added

- Capture a tmux pane as a cell grid and render it at TV resolution, redrawing
  only when the content actually changes.
- `browser` backend: MJPEG over HTTP, no external tools required.
- `dlna` backend: find a MediaRenderer over SSDP and hand it an H.264/MPEG-TS
  URL with `SetAVTransportURI` + `Play`.
- `miracast` backend: RTP/MPEG-TS transport plus the Wi-Fi Display RTSP
  capability negotiation. Experimental, and unverified end to end.
- `preview` backend: write frames to a PNG.
- `ttycast doctor`, `ttycast discover`, `ttycast backends`.
- H.264 encoder autodetection across `libx264`, `libopenh264` and `h264_vaapi`,
  so distributions shipping a patent-free ffmpeg work without extra packages.

[Unreleased]: https://github.com/michaelkrisper/ttycast/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/michaelkrisper/ttycast/releases/tag/v0.1.0
