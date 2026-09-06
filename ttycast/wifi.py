"""Can this machine even do Wi-Fi Direct?

Miracast rides on Wi-Fi Direct, and Wi-Fi Direct needs a driver that offers
``P2P-client``/``P2P-GO`` interface modes. Plenty of laptops - anything on
Broadcom's proprietary ``wl``, for one - never do, and no amount of userspace
can work around it. Better to say so up front than to fail deep in a handshake.
"""

from __future__ import annotations

import re
import shutil
import subprocess
from dataclasses import dataclass

_MODES_BLOCK = re.compile(r"Supported interface modes:\s*((?:\s*\*\s*\S+\n?)+)")


@dataclass(frozen=True, slots=True)
class Phy:
    name: str
    modes: tuple[str, ...]
    driver: str = ""

    @property
    def supports_p2p(self) -> bool:
        return any(mode.startswith("P2P-") for mode in self.modes)


def parse_iw_list(text: str) -> list[Phy]:
    """Pull ``(phy, supported modes)`` out of ``iw list`` / ``iw phy info``."""
    phys: list[Phy] = []
    for chunk in re.split(r"^Wiphy\s+", text, flags=re.M)[1:]:
        name = chunk.split("\n", 1)[0].strip()
        match = _MODES_BLOCK.search(chunk)
        modes = ()
        if match:
            modes = tuple(
                line.strip().lstrip("*").strip()
                for line in match.group(1).splitlines()
                if line.strip()
            )
        phys.append(Phy(name=name, modes=modes))
    return phys


def driver_of(phy: str) -> str:
    """Best-effort driver name via sysfs; empty string when it cannot be read."""
    try:
        with open(f"/sys/class/ieee80211/{phy}/device/uevent") as handle:
            for line in handle:
                if line.startswith("DRIVER="):
                    return line.strip().split("=", 1)[1]
    except OSError:
        pass
    return ""


def phys() -> list[Phy]:
    if shutil.which("iw") is None:
        return []
    try:
        out = subprocess.run(  # noqa: S603
            ["iw", "list"], capture_output=True, text=True, timeout=10
        )
    except (OSError, subprocess.SubprocessError):
        return []
    return [Phy(p.name, p.modes, driver_of(p.name)) for p in parse_iw_list(out.stdout)]


def p2p_report() -> tuple[bool, list[str]]:
    """``(usable, explanation lines)`` for the doctor and for preflight."""
    found = phys()
    if not found:
        return False, ["no wireless phy found (is 'iw' installed and a wifi card present?)"]
    lines = []
    usable = False
    for phy in found:
        modes = ", ".join(phy.modes) or "unknown"
        driver = f" driver={phy.driver}" if phy.driver else ""
        if phy.supports_p2p:
            usable = True
            lines.append(f"{phy.name}{driver}: P2P supported ({modes})")
        else:
            lines.append(f"{phy.name}{driver}: no P2P mode ({modes})")
    if not usable:
        lines.append(
            "Miracast needs Wi-Fi Direct. Fix: a USB wifi adapter whose driver offers "
            "P2P-GO/P2P-client (mt76 e.g. MT7612U, rtw88, or an Intel AX2xx)."
        )
    return usable, lines
