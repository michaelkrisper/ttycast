# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
