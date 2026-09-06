# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-09-06

Rewritten in Rust. Same features, same CLI, one static binary.

### Changed

- **Rewritten from Python to Rust.** Full-screen scrolling went from 34 fps to
  121 fps end to end on a 2013 dual-core laptop, rendering from 14.3 ms to
  1.4 ms per frame, ANSI parsing from 2.2 ms to 0.03 ms, and process startup
  from 185 ms to 1.3 ms. The result is a ~2 MB binary with no runtime to
  install.
- Glyphs are rasterised with fontdue into coverage masks and blitted by hand;
  the row diffing, single-invocation tmux capture and raw-capture change
  detection from 0.1 are all preserved, and the renderer is still tested
  against the property that incremental output is byte-identical to a full
  repaint.
- JPEG encoding uses mozjpeg with `set_fastest_defaults()` and 4:2:0 chroma.
  The pure-Rust encoders measured three times slower than libjpeg-turbo, and
  mozjpeg's own defaults measured 90 ms per 720p frame because it optimises for
  file size; wound back, it beats the previous Pillow path at 3.3 ms.
- The HTTP server, the SSDP/SOAP control point and the RTSP message layer are
  hand-rolled on `std::net`. Dependencies are `clap`, `fontdue`, `mozjpeg`,
  `png` and `libc`.
- SIGINT, SIGTERM and SIGHUP set a flag rather than killing the process, so the
  screen is restored and the TV is told to stop on the way out. SIGPIPE is
  ignored so a client vanishing mid-write cannot take the process down.

### Fixed

- The UPnP tag reader stripped the namespace before splitting off the element
  name, so a tag with a namespaced attribute (`xmlns:dlna="urn:a:b"`) matched
  against the attribute value instead of the element.

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
