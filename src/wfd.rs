//! The Wi-Fi Display (Miracast) source side of the RTSP handshake.
//!
//! Miracast is three layers stacked: Wi-Fi Direct carries the link, RTSP
//! negotiates the session (the M1..M7 exchange below), and the picture travels
//! as H.264 inside MPEG-TS over RTP/UDP. Only the middle layer lives here, as
//! pure message building and parsing, so it can be tested without a radio.
//!
//! The link layer is not implemented here - see [`crate::backend::miracast`],
//! which leaves peer association to wpa_supplicant or MiracleCast.
//!
//! Reference: Wi-Fi Alliance "Wi-Fi Display Technical Specification" v2.x,
//! section 6.4 (Capability Negotiation) and RFC 2326 (RTSP 1.0).

// The RTSP layer is complete and tested, but nothing drives it yet: the
// miracast backend still needs an already-associated sink. Keeping it here,
// exercised by its tests, is what makes the handshake reviewable before there
// is hardware to try it on.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

pub const RTSP_VERSION: &str = "RTSP/1.0";
pub const USER_AGENT: &str = "ttycast/0.2";

/// The CEA resolution bitmaps a source advertises in `wfd_video_formats`.
pub const CEA_640X480P60: u32 = 1 << 0;
pub const CEA_720X480P60: u32 = 1 << 1;
pub const CEA_1280X720P30: u32 = 1 << 5;
pub const CEA_1280X720P60: u32 = 1 << 6;
pub const CEA_1920X1080P30: u32 = 1 << 8;

/// `(bitmap, width, height, fps)` for the modes ttycast is willing to send,
/// best first. Only progressive modes every sink must support.
pub const SUPPORTED_MODES: [(u32, u32, u32, u32); 4] = [
    (CEA_1920X1080P30, 1920, 1080, 30),
    (CEA_1280X720P60, 1280, 720, 60),
    (CEA_1280X720P30, 1280, 720, 30),
    (CEA_640X480P60, 640, 480, 60),
];

/// An RTSP request or response, either direction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    /// `("OPTIONS", "rtsp://...")` for a request, `None` for a response.
    pub request: Option<(String, String)>,
    /// `(code, reason)` for a response, `None` for a request.
    pub status: Option<(u16, String)>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Message {
    #[must_use]
    pub fn cseq(&self) -> Option<u32> {
        self.header("cseq")?.parse().ok()
    }

    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let mut headers = self.headers.clone();
        if !self.body.is_empty() {
            if !headers.iter().any(|(k, _)| k == "content-type") {
                headers.push(("content-type".into(), "text/parameters".into()));
            }
            headers.push(("content-length".into(), self.body.len().to_string()));
        }
        let start = if let Some((method, url)) = &self.request {
            format!("{method} {url} {RTSP_VERSION}")
        } else {
            let (code, reason) = self.status.clone().unwrap_or((200, "OK".into()));
            format!("{RTSP_VERSION} {code} {reason}")
        };
        let mut out = start;
        for (key, value) in &headers {
            let _ = write!(out, "\r\n{}: {value}", canonical(key));
        }
        out.push_str("\r\n\r\n");
        out.push_str(&self.body);
        out
    }
}

/// RTSP headers are case-insensitive, but sinks in the wild are picky.
#[must_use]
pub fn canonical(name: &str) -> String {
    if name.starts_with("wfd_") {
        return name.to_string();
    }
    if name.eq_ignore_ascii_case("cseq") {
        return "CSeq".to_string();
    }
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
            })
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse one complete RTSP message; the head must already be terminated.
#[must_use]
pub fn parse(raw: &str) -> Message {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.split("\r\n");
    let start: Vec<&str> = lines.next().unwrap_or("").splitn(3, ' ').collect();
    let mut message = Message {
        body: body.to_string(),
        ..Message::default()
    };
    if start.first().is_some_and(|s| s.starts_with("RTSP/")) {
        message.status = Some((
            start.get(1).and_then(|c| c.parse().ok()).unwrap_or(0),
            start.get(2).unwrap_or(&"").to_string(),
        ));
    } else {
        message.request = Some((
            start.first().unwrap_or(&"").to_string(),
            start.get(1).unwrap_or(&"*").to_string(),
        ));
    }
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            message
                .headers
                .push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    message
}

/// `wfd_audio_codecs: LPCM 00000002 00` style bodies.
#[must_use]
pub fn parse_parameters(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.lines() {
        if let Some((key, value)) = line.split_once(':') {
            out.insert(key.trim().to_string(), value.trim().to_string());
        } else if !line.trim().is_empty() {
            out.insert(line.trim().to_string(), String::new());
        }
    }
    out
}

#[must_use]
pub fn format_parameters(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(key, value)| {
            if value.is_empty() {
                format!("{key}\r\n")
            } else {
                format!("{key}: {value}\r\n")
            }
        })
        .collect()
}

#[must_use]
pub fn response(cseq: u32, extra: &[(&str, &str)]) -> Message {
    let mut headers = vec![("cseq".to_string(), cseq.to_string())];
    headers.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
    );
    Message {
        status: Some((200, "OK".into())),
        headers,
        ..Message::default()
    }
}

#[must_use]
pub fn request(method: &str, url: &str, cseq: u32, body: &str, extra: &[(&str, &str)]) -> Message {
    let mut headers = vec![
        ("cseq".to_string(), cseq.to_string()),
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ];
    headers.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
    );
    Message {
        request: Some((method.to_string(), url.to_string())),
        headers,
        body: body.to_string(),
        ..Message::default()
    }
}

/// M1: the sink asks what we support.
#[must_use]
pub fn options_response(cseq: u32) -> Message {
    response(
        cseq,
        &[("public", "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER")],
    )
}

/// M2: we ask the same of the sink.
#[must_use]
pub fn options_request(url: &str, cseq: u32) -> Message {
    request("OPTIONS", url, cseq, "", &[("require", "org.wfa.wfd1.0")])
}

/// M3: ask the sink which formats and ports it can take.
#[must_use]
pub fn get_parameter_request(url: &str, cseq: u32) -> Message {
    let body = format_parameters(&[
        ("wfd_video_formats", ""),
        ("wfd_audio_codecs", ""),
        ("wfd_client_rtp_ports", ""),
        ("wfd_content_protection", ""),
    ]);
    request("GET_PARAMETER", url, cseq, &body, &[])
}

/// The `wfd_video_formats` value for a single offered CEA mode.
///
/// Layout: native, preferred-display-mode, then one profile block of
/// `profile latency cea vesa hh min-slice slice-enc frame-skip max-hres max-vres`.
#[must_use]
pub fn video_formats_value(bitmap: u32) -> String {
    format!("00 00 02 04 {bitmap:08X} 00000000 00000000 00 0000 0000 00 none none")
}

/// Pick the best mode both sides support: `(width, height, fps)`.
///
/// `sink_video_formats` is the raw value the sink sent in its M3 reply. Only
/// the CEA bitmap is considered; VESA and handheld modes exist but no TV needs
/// them.
#[must_use]
pub fn choose_mode(sink_video_formats: &str) -> Option<(u32, u32, u32)> {
    let fields: Vec<&str> = sink_video_formats.split_whitespace().collect();
    let cea = u32::from_str_radix(fields.get(4)?, 16).ok()?;
    SUPPORTED_MODES
        .iter()
        .find(|(bitmap, ..)| cea & bitmap != 0)
        .map(|&(_, w, h, fps)| (w, h, fps))
}

/// M4: tell the sink the format and where the RTP will come from.
#[must_use]
pub fn set_parameter_body(rtp_port: u16, bitmap: u32) -> String {
    let formats = video_formats_value(bitmap);
    let ports = format!("RTP/AVP/UDP;unicast {rtp_port} 0 mode=play");
    format_parameters(&[
        ("wfd_video_formats", &formats),
        ("wfd_audio_codecs", "none"),
        (
            "wfd_presentation_URL",
            "rtsp://localhost/wfd1.0/streamid=0 none",
        ),
        ("wfd_client_rtp_ports", &ports),
    ])
}

/// M5: `SETUP`, `PLAY`, `PAUSE` or `TEARDOWN`.
#[must_use]
pub fn trigger_body(method: &str) -> String {
    format_parameters(&[("wfd_trigger_method", method)])
}

/// Pull the sink's RTP port out of `wfd_client_rtp_ports`.
#[must_use]
pub fn parse_client_rtp_ports(value: &str) -> Option<u16> {
    value
        .split_whitespace()
        .filter_map(|token| token.parse::<u16>().ok())
        .find(|&port| port != 0)
}

/// `RTP/AVP/UDP;unicast;client_port=19000-19001` -> 19000.
#[must_use]
pub fn parse_transport_port(transport: &str) -> Option<u16> {
    transport
        .split(';')
        .find_map(|part| part.trim().strip_prefix("client_port="))
        .and_then(|value| value.split('-').next())
        .and_then(|first| first.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        let message = options_request("rtsp://192.168.49.1/wfd1.0", 1);
        let raw = message.encode();
        assert!(raw.starts_with("OPTIONS rtsp://192.168.49.1/wfd1.0 RTSP/1.0\r\n"));
        assert!(raw.contains("CSeq: 1\r\n"));
        let parsed = parse(&raw);
        assert_eq!(
            parsed.request,
            Some(("OPTIONS".into(), "rtsp://192.168.49.1/wfd1.0".into()))
        );
        assert_eq!(parsed.cseq(), Some(1));
    }

    #[test]
    fn a_response_round_trips() {
        let raw = options_response(4).encode();
        assert!(raw.starts_with("RTSP/1.0 200 OK\r\n"));
        assert!(raw.contains("org.wfa.wfd1.0"));
        assert_eq!(parse(&raw).status, Some((200, "OK".into())));
    }

    #[test]
    fn content_length_matches_the_body() {
        let raw = get_parameter_request("rtsp://x", 2).encode();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        assert!(head.contains(&format!("Content-Length: {}", body.len())));
        assert!(body.contains("wfd_video_formats"));
    }

    #[test]
    fn wfd_headers_keep_their_lowercase_names() {
        let message = Message {
            request: Some(("SET_PARAMETER".into(), "rtsp://x".into())),
            headers: vec![("wfd_trigger".into(), "SETUP".into())],
            ..Message::default()
        };
        assert!(message.encode().contains("wfd_trigger: SETUP"));
    }

    #[test]
    fn parse_parameters_handles_valued_and_bare_keys() {
        let params =
            parse_parameters("wfd_video_formats: 00 00 02 04 0001DEFF\r\nwfd_audio_codecs\r\n");
        assert!(params["wfd_video_formats"].starts_with("00 00 02 04"));
        assert_eq!(params["wfd_audio_codecs"], "");
    }

    #[test]
    fn format_parameters_is_the_inverse() {
        let text = format_parameters(&[("wfd_trigger_method", "SETUP"), ("wfd_audio_codecs", "")]);
        let params = parse_parameters(&text);
        assert_eq!(params["wfd_trigger_method"], "SETUP");
        assert_eq!(params["wfd_audio_codecs"], "");
    }

    #[test]
    fn choose_mode_prefers_the_largest_common_format() {
        let both = CEA_1280X720P30 | CEA_1920X1080P30;
        let formats = format!("00 00 02 04 {both:08X} 00000000 00000000 00 0000 0000 00 none none");
        assert_eq!(choose_mode(&formats), Some((1920, 1080, 30)));
    }

    #[test]
    fn choose_mode_falls_back_to_a_smaller_format() {
        let formats = format!("00 00 02 04 {CEA_1280X720P30:08X} 00000000 00000000");
        assert_eq!(choose_mode(&formats), Some((1280, 720, 30)));
    }

    #[test]
    fn choose_mode_returns_none_when_nothing_matches() {
        assert_eq!(choose_mode("00 00 02 04 00000000 00000000 00000000"), None);
        assert_eq!(choose_mode("garbage"), None);
        assert_eq!(choose_mode("00 00 02 04 ZZZZ"), None);
    }

    #[test]
    fn video_formats_value_encodes_the_bitmap() {
        let value = video_formats_value(CEA_1280X720P30);
        assert_eq!(
            value.split_whitespace().nth(4).unwrap(),
            format!("{CEA_1280X720P30:08X}")
        );
    }

    #[test]
    fn set_parameter_body_carries_the_rtp_port() {
        let params = parse_parameters(&set_parameter_body(19000, CEA_1280X720P30));
        assert!(params["wfd_client_rtp_ports"].contains("19000"));
        assert_eq!(params["wfd_audio_codecs"], "none");
    }

    #[test]
    fn trigger_body_names_the_method() {
        assert_eq!(
            parse_parameters(&trigger_body("PLAY"))["wfd_trigger_method"],
            "PLAY"
        );
    }

    #[test]
    fn client_rtp_ports_are_extracted() {
        assert_eq!(
            parse_client_rtp_ports("RTP/AVP/UDP;unicast 19000 0 mode=play"),
            Some(19000)
        );
        assert_eq!(
            parse_client_rtp_ports("RTP/AVP/UDP;unicast 0 0 mode=play"),
            None
        );
    }

    #[test]
    fn transport_ports_are_extracted() {
        assert_eq!(
            parse_transport_port("RTP/AVP/UDP;unicast;client_port=19000-19001"),
            Some(19000)
        );
        assert_eq!(parse_transport_port("RTP/AVP/UDP;unicast"), None);
    }
}
