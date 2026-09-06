from ttycast import wfd


def test_request_round_trips():
    message = wfd.request("OPTIONS", "rtsp://192.168.49.1/wfd1.0", 1)
    raw = message.encode().decode()
    assert raw.startswith("OPTIONS rtsp://192.168.49.1/wfd1.0 RTSP/1.0\r\n")
    assert "CSeq: 1\r\n" in raw
    parsed = wfd.parse(raw)
    assert parsed.request == ("OPTIONS", "rtsp://192.168.49.1/wfd1.0")
    assert parsed.cseq == 1


def test_response_round_trips():
    raw = wfd.options_response(4).encode().decode()
    assert raw.startswith("RTSP/1.0 200 OK\r\n")
    assert "org.wfa.wfd1.0" in raw
    assert wfd.parse(raw).status == (200, "OK")


def test_content_length_matches_the_body():
    message = wfd.get_parameter_request("rtsp://x", 2)
    raw = message.encode().decode()
    head, _, body = raw.partition("\r\n\r\n")
    assert f"Content-Length: {len(body.encode())}" in head
    assert "wfd_video_formats" in body


def test_wfd_headers_keep_their_lowercase_names():
    message = wfd.Message(request=("SET_PARAMETER", "rtsp://x"), headers={"wfd_trigger": "SETUP"})
    assert "wfd_trigger: SETUP" in message.encode().decode()


def test_parse_parameters_handles_valued_and_bare_keys():
    body = "wfd_video_formats: 00 00 02 04 0001DEFF\r\nwfd_audio_codecs\r\n"
    params = wfd.parse_parameters(body)
    assert params["wfd_video_formats"].startswith("00 00 02 04")
    assert params["wfd_audio_codecs"] == ""


def test_format_parameters_is_the_inverse():
    params = {"wfd_trigger_method": "SETUP", "wfd_audio_codecs": ""}
    assert wfd.parse_parameters(wfd.format_parameters(params)) == params


def test_choose_mode_prefers_the_largest_common_format():
    both = wfd.CEA_1280x720p30 | wfd.CEA_1920x1080p30
    formats = f"00 00 02 04 {both:08X} 00000000 00000000 00 0000 0000 00 none none"
    assert wfd.choose_mode(formats) == (1920, 1080, 30)


def test_choose_mode_falls_back_to_a_smaller_format():
    formats = f"00 00 02 04 {wfd.CEA_1280x720p30:08X} 00000000 00000000"
    assert wfd.choose_mode(formats) == (1280, 720, 30)


def test_choose_mode_returns_none_when_nothing_matches():
    assert wfd.choose_mode("00 00 02 04 00000000 00000000 00000000") is None
    assert wfd.choose_mode("garbage") is None
    assert wfd.choose_mode("00 00 02 04 ZZZZ") is None


def test_video_formats_value_encodes_the_bitmap():
    value = wfd.video_formats_value(wfd.CEA_1280x720p30)
    assert value.split()[4] == f"{wfd.CEA_1280x720p30:08X}"


def test_set_parameter_body_carries_the_rtp_port():
    body = wfd.set_parameter_body(19000, wfd.CEA_1280x720p30)
    params = wfd.parse_parameters(body)
    assert "19000" in params["wfd_client_rtp_ports"]
    assert params["wfd_audio_codecs"] == "none"


def test_trigger_body():
    assert wfd.parse_parameters(wfd.trigger_body("PLAY")) == {"wfd_trigger_method": "PLAY"}


def test_parse_client_rtp_ports():
    assert wfd.parse_client_rtp_ports("RTP/AVP/UDP;unicast 19000 0 mode=play") == 19000
    assert wfd.parse_client_rtp_ports("RTP/AVP/UDP;unicast 0 0 mode=play") is None


def test_parse_transport_port():
    assert wfd.parse_transport_port("RTP/AVP/UDP;unicast;client_port=19000-19001") == 19000
    assert wfd.parse_transport_port("RTP/AVP/UDP;unicast") is None
