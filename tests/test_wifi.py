from ttycast import wifi

NO_P2P = """Wiphy phy0
\tmax # scan SSIDs: 10
\tSupported interface modes:
\t\t * IBSS
\t\t * managed
\t\t * AP
\tBand 1:
"""

WITH_P2P = """Wiphy phy0
\tSupported interface modes:
\t\t * managed
\t\t * AP
\t\t * P2P-client
\t\t * P2P-GO
\t\t * P2P-device
\tBand 1:
Wiphy phy1
\tSupported interface modes:
\t\t * monitor
"""


def test_parse_picks_up_modes():
    phys = wifi.parse_iw_list(NO_P2P)
    assert len(phys) == 1
    assert phys[0].name == "phy0"
    assert phys[0].modes == ("IBSS", "managed", "AP")


def test_p2p_detection():
    assert wifi.parse_iw_list(NO_P2P)[0].supports_p2p is False
    phys = wifi.parse_iw_list(WITH_P2P)
    assert phys[0].supports_p2p is True
    assert phys[1].supports_p2p is False


def test_multiple_phys_are_all_returned():
    assert [p.name for p in wifi.parse_iw_list(WITH_P2P)] == ["phy0", "phy1"]


def test_empty_input_yields_nothing():
    assert wifi.parse_iw_list("") == []


def test_report_explains_the_fix_when_p2p_is_missing(monkeypatch):
    monkeypatch.setattr(wifi, "phys", lambda: wifi.parse_iw_list(NO_P2P))
    usable, lines = wifi.p2p_report()
    assert usable is False
    assert any("USB wifi adapter" in line for line in lines)


def test_report_is_happy_with_a_p2p_adapter(monkeypatch):
    monkeypatch.setattr(wifi, "phys", lambda: wifi.parse_iw_list(WITH_P2P))
    usable, lines = wifi.p2p_report()
    assert usable is True
    assert any("P2P supported" in line for line in lines)


def test_report_without_any_adapter(monkeypatch):
    monkeypatch.setattr(wifi, "phys", list)
    usable, lines = wifi.p2p_report()
    assert usable is False
    assert "no wireless phy found" in lines[0]
