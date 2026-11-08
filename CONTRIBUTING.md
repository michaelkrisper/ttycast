# Contributing

Bug reports and patches are welcome, especially reports from real televisions.

## Workflow

1. Every change starts with an issue: a bug report or a feature request.
2. Work on a branch named after the issue. `gh issue develop <issue> --checkout`
   creates `<issue>-<title>`.
3. Open a pull request against `main`. The _Tickets_ check links the issue
   (`Closes #<issue>`); a pull request without a ticket fails.
4. CI runs on every pull request. When all checks are green, _Auto-merge_
   squash-merges it. Open the pull request as a draft while it is not ready.
5. The merge closes the issue. `main` always holds the latest valid state
   (trunk-based development).

## Versions and releases

- Versions follow [Semantic Versioning](https://semver.org) and exist only as
  git tags (`vMAJOR.MINOR.PATCH`).
- CI tags every merge to `main` with the next patch version.
- A release bundles the changes since the previous one: run
  _Actions → Release → Run workflow_ and pick `minor` or `major`. It tags
  `main` and publishes a GitHub release listing the merged pull requests and
  closed tickets.

## Setup

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Building needs `nasm`, because mozjpeg compiles libjpeg-turbo's SIMD kernels.

## What a good report looks like

For anything involving a TV, the useful details are:

- the output of `ttycast doctor`
- make, model and firmware of the set
- which backend you used and the exact command line
- for DLNA: the output of `ttycast discover`
- for Miracast: the sink's `wfd_video_formats` reply, if you can get at it

## House rules

- **No new dependencies** without a good reason. There are five - `clap`,
  `fontdue`, `mozjpeg`, `png`, `libc` - and ffmpeg is an external tool, not a
  crate. The HTTP server and the UPnP client are hand-rolled on purpose.
- **Performance claims come with a measurement.** Every number in the README was
  measured on the machine it names; if you change a hot path, measure it again
  and say what you measured on.
- **Protocol code stays free of I/O.** `ansi`, `wfd`, `upnp` and `wifi` are
  message building and parsing; sockets and subprocesses live elsewhere. This is
  what makes them testable without hardware.
- **Do not paste code from GPL projects.** ttycast is MIT, and it needs to stay
  usable in commercial work. Protocol facts from a specification are fine;
  copied implementations are not.
- Every bug fix comes with the test that would have caught it.

## Adding a backend

Implement the `Backend` trait in a new `src/backend/*.rs`, filling in what you
need of `preflight` / `start` / `on_frame` / `status` / `stop`, then add it to
`create` and `SUMMARIES` in `src/backend/mod.rs`.

`preflight` should return a *actionable* problem list: not "failed" but what is
missing and what would fix it. That is the difference between a tool people can
debug and one they give up on.
