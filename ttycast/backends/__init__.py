"""Backend registry."""

from __future__ import annotations

from ttycast.backends.base import Backend, BackendError, Context
from ttycast.backends.browser import BrowserBackend
from ttycast.backends.dlna import DlnaBackend
from ttycast.backends.miracast import MiracastBackend
from ttycast.backends.preview import PreviewBackend

#: Ordered so that ``--help`` lists the most likely choice first.
REGISTRY: dict[str, type[Backend]] = {
    cls.name: cls for cls in (BrowserBackend, DlnaBackend, MiracastBackend, PreviewBackend)
}


def create(name: str) -> Backend:
    try:
        return REGISTRY[name]()
    except KeyError:
        known = ", ".join(REGISTRY)
        raise BackendError(f"unknown backend {name!r} (known: {known})") from None


__all__ = ["REGISTRY", "Backend", "BackendError", "Context", "create"]
