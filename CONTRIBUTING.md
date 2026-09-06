# Contributing

Bug reports and patches are welcome, especially reports from real televisions.

## Setup

```bash
python -m venv .venv
.venv/bin/pip install -e '.[dev]'
.venv/bin/pytest
.venv/bin/ruff check .
.venv/bin/ruff format --check .
```

## What a good report looks like

For anything involving a TV, the useful details are:

- the output of `ttycast doctor`
- make, model and firmware of the set
- which backend you used and the exact command line
- for DLNA: the output of `ttycast discover`
- for Miracast: the sink's `wfd_video_formats` reply, if you can get at it

## House rules

- **No new runtime dependencies** without a good reason. Pillow is the only one,
  and ffmpeg is an external tool, not a Python package.
- **Protocol code stays free of I/O.** `ansi`, `wfd`, `upnp` and `wifi` are
  message building and parsing; sockets and subprocesses live elsewhere. This is
  what makes them testable without hardware.
- **Do not paste code from GPL projects.** ttycast is MIT, and it needs to stay
  usable in commercial work. Protocol facts from a specification are fine;
  copied implementations are not.
- Every bug fix comes with the test that would have caught it.

## Adding a backend

Subclass `ttycast.backends.base.Backend`, implement what you need of
`preflight` / `start` / `on_frame` / `status` / `stop`, and add the class to
`REGISTRY` in `ttycast/backends/__init__.py`.

`preflight` should return a *actionable* problem list: not "failed" but what is
missing and what would fix it. That is the difference between a tool people can
debug and one they give up on.
