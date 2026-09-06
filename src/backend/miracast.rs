//! Wireless display over Miracast.
//!
//! This is the only backend that mirrors rather than plays a file, so it is
//! the only one with sub-second latency - the right choice for actually
//! working at the TV. The cost is that it needs Wi-Fi Direct in the driver.
//!
//! ttycast does not reimplement the Wi-Fi Direct link layer. It produces the
//! picture (H.264 in MPEG-TS over RTP, exactly what Miracast carries) and
//! leaves peer discovery and association to wpa_supplicant, driven either
//! directly or through MiracleCast. The RTSP capability negotiation lives in
//! [`crate::wfd`].
//!
//! Status: the RTSP layer and the RTP transport are implemented and
//! unit-tested; the end-to-end path is unverified because no P2P-capable radio
//! was available during development. Treat it as experimental.

use super::{Backend, Context};
use crate::capture::which;
use crate::encoder::{RtpSender, have_ffmpeg, no_encoder_message, pick_encoder};
use crate::wifi::p2p_report;

/// Userspace helpers that can drive the Wi-Fi Direct side for us.
const LINK_HELPERS: [&str; 2] = ["miracle-wifid", "gnome-network-displays"];

#[derive(Default)]
pub struct MiracastBackend {
    sender: Option<RtpSender>,
}

impl Backend for MiracastBackend {
    fn name(&self) -> &'static str {
        "miracast"
    }

    fn preflight(&self, ctx: &Context) -> Vec<String> {
        let mut problems = Vec::new();
        if !have_ffmpeg() {
            problems.push("ffmpeg not found on PATH (needed to produce the H.264 stream)".into());
        } else if pick_encoder(ctx.encoder.as_deref()).is_none() {
            problems.push(no_encoder_message());
        }

        // A pre-negotiated sink means someone else already did the link layer,
        // so the hardware check does not apply.
        if ctx.sink_host.is_some() {
            return problems;
        }

        let (usable, lines) = p2p_report();
        if !usable {
            problems.push("no Wi-Fi Direct capable adapter:".into());
            problems.extend(lines.into_iter().map(|line| format!("  {line}")));
        }
        if !LINK_HELPERS.iter().any(|helper| which(helper).is_some()) {
            problems.push(
                "no Wi-Fi Direct helper found. Install MiracleCast (miracle-wifid) or \
                 gnome-network-displays, or pass --sink-host/--sink-port to skip discovery \
                 and send RTP to an already connected sink."
                    .into(),
            );
        }
        problems
    }

    fn start(&mut self, ctx: &Context) -> Result<(), String> {
        let Some(host) = ctx.sink_host.as_ref() else {
            return Err("automatic Miracast session setup is not wired up yet. Connect the sink \
                        with MiracleCast, then run ttycast with --sink-host <ip> --sink-port <port>."
                .into());
        };
        let encoder = pick_encoder(ctx.encoder.as_deref()).ok_or_else(no_encoder_message)?;
        let sender = RtpSender::new(
            ctx.width,
            ctx.height,
            ctx.fps,
            host,
            ctx.sink_port,
            &ctx.bitrate,
            &encoder,
        );
        sender.start()?;
        ctx.log(&format!("sending RTP/MPEG-TS to {host}:{}", ctx.sink_port));
        self.sender = Some(sender);
        Ok(())
    }

    fn on_frame(&mut self, ctx: &Context, _changed: bool) {
        if let (Some(sender), Some(server)) = (self.sender.as_ref(), ctx.server.as_ref())
            && let Some(frame) = server.bus().latest()
        {
            sender.write_frame(&frame);
        }
    }

    fn status(&self, ctx: &Context) -> String {
        match (&self.sender, &ctx.sink_host) {
            (Some(_), Some(host)) => format!("rtp -> {host}:{}", ctx.sink_port),
            _ => "idle".into(),
        }
    }

    fn stop(&mut self, _ctx: &Context) {
        if let Some(sender) = self.sender.take() {
            sender.stop();
        }
    }
}
