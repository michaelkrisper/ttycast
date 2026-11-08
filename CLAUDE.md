# CLAUDE.md

ttycast mirrors a tmux pane to a TV: it reads the pane as text (`tmux capture-pane -e`),
renders it at TV resolution with its own glyph cache and serves it as MJPEG to a browser
(default), as H.264/MPEG-TS to a DLNA renderer, or as H.264/RTP to a Miracast sink.
Rust 2024, MSRV 1.88, one binary, no daemon, no config file. Nothing is deployed; users
install with `cargo install --git`. The README is thorough on behaviour, backends and
measurements; read it instead of re-deriving.

## Commands

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
cargo check --locked                  # what the MSRV job runs on 1.88 (binary only)
cargo run -- doctor                   # what works on this machine
cargo run -- --once --backend preview # render one frame to a PNG, no TV needed
```

- Building needs `nasm` (mozjpeg compiles libjpeg-turbo's SIMD kernels).
- CI installs `nasm tmux fontconfig fonts-dejavu-core`; the font tests need fontconfig
  and a monospace font, so a failure there on a bare machine is environmental.
- CI (`.github/workflows/ci.yml`): job `test` runs fmt, clippy, test, release build;
  job `msrv` runs `cargo check --locked` on 1.88. Dev-dependencies may need a newer
  toolchain, so the MSRV job deliberately skips tests.
- Clippy runs with `all` + `pedantic` as warnings (see `[lints.clippy]` in `Cargo.toml`)
  and CI turns warnings into errors.

## Architecture

```
capture.rs (tmux) -> ansi.rs / status.rs (cell grid) -> render.rs (RGB frame, row diff)
  -> framebus.rs (latest frame) -> server.rs (MJPEG, /live.ts) | encoder.rs (ffmpeg H.264)
```

- `src/main.rs`: clap CLI and subcommands (`doctor`, `discover`, `screen-on`).
- `src/backend/`: one file per backend behind the `Backend` trait (`preflight`, `start`,
  `on_frame`, `status`, `stop`); register new ones in `create` and `SUMMARIES` in
  `src/backend/mod.rs`. Backends never touch capture or rendering.
- Protocol layers `ansi`, `wfd` (Miracast RTSP), `upnp` (SSDP/SOAP), `wifi` are pure
  message building/parsing without I/O; sockets and subprocesses live elsewhere.
- `display.rs`: `--screen-off` (sway/wlopm/xset/backlight). `doctor.rs`: environment check.
- `build.rs` sets `TTYCAST_VERSION` from `git describe --tags` (falls back to the
  `Cargo.toml` version outside a git checkout); `main.rs` uses it for `--version`.

## Conventions

- `rustfmt.toml`: max width 100. Comments explain why, not what.
- Only five dependencies (`clap`, `fontdue`, `mozjpeg`, `png`, `libc`). Do not add crates
  without a strong reason; the HTTP server and UPnP client are hand-rolled on purpose.
  ffmpeg is an external tool found on `PATH`, never a crate.
- Every bug fix comes with the test that would have caught it. `render` is tested against
  the invariant that incremental output equals a full repaint, byte for byte.
- Performance claims in the README name the machine they were measured on. If you change
  a hot path, measure again and update the number with its machine; never guess one.
- `preflight` returns actionable problems (what is missing, what fixes it), not "failed".

## Rules and pitfalls

- Never copy code from GPL projects; the crate is MIT and must stay usable commercially.
  Protocol facts from a specification are fine, implementations are not.
- Never edit the version in `Cargo.toml` (0.2.0); it is only the fallback for builds
  without git. `CHANGELOG.md` ends at 0.2.1; later changes live in GitHub releases.
- SSDP discovery must bind UDP 1900 with `SO_REUSEADDR` + `SO_REUSEPORT` (raw `libc`,
  std cannot set them before bind), falling back to an ephemeral port. Some TVs (Hisense
  VIDAA) unicast M-SEARCH replies to 1900 regardless of the source port; from an
  ephemeral port discovery silently finds nothing (`upnp.rs::ssdp_socket`).
- DLNA latency of several seconds is the renderer's buffer, not ttycast; ffmpeg emits
  its first byte within ~0.25 s. Do not try to tune it away on the sender side.
- A renderer usually only appears while the TV's content-sharing setting is on.
- Miracast needs a Wi-Fi Direct (P2P-GO) capable driver; it is experimental and
  unverified end to end. Session setup through wpa_supplicant is not wired up.
- Glyph size follows the pane's row count, not its width (`tmux resize-window -y N`).
- Cell height comes from FULL BLOCK (U+2588), not font line metrics, so box drawing
  tiles; JPEG uses 4:4:4 chroma on purpose. Do not "optimise" either back.
- Try things inside a throwaway tmux server (`tmux -L test`). Outside tmux, ttycast
  creates a `ttycast` session on the default server; never kill or reconfigure the
  default server, it may hold someone's live session.

## Workflow

- Every change starts with a GitHub issue. Branch `<issue>-<slug>` from `main`,
  open a PR with `Closes #<issue>`. Never push to `main` directly.
- PRs auto-merge (squash) once CI and the "Linked ticket" check pass; drafts are not merged.
- Versions exist only as git tags `vX.Y.Z`. Every merge to `main` tags the next patch;
  never edit version numbers in files. Minor/major releases: Actions -> Release.
- Nothing deploys automatically; users install from git with `cargo install --git`.
- Details: CONTRIBUTING.md.
