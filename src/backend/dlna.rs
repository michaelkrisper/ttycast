//! Push the stream to a DLNA MediaRenderer (most smart TVs).
//!
//! The TV is handed a URL into our own HTTP server and pulls the transport
//! stream itself. Expect one to several seconds of latency: renderers buffer,
//! and that is not something a sender can turn off. Fine for watching a build
//! or a log, awkward for typing.
//!
//! If discovery comes back empty, the usual cause is the TV: content/screen
//! sharing has to be enabled in its settings before the renderer is announced.

use std::time::Duration;

use super::{Backend, Context};
use crate::encoder::{have_ffmpeg, no_encoder_message, pick_encoder};
use crate::upnp;

#[derive(Default)]
pub struct DlnaBackend {
    renderer: Option<upnp::Renderer>,
}

impl DlnaBackend {
    fn choose(ctx: &Context) -> Result<upnp::Renderer, String> {
        ctx.log("searching for a DLNA renderer ...");
        let renderers = upnp::discover(Duration::from_secs_f64(ctx.discovery_timeout));
        if renderers.is_empty() {
            return Err(
                "no DLNA renderer answered. Switch the TV on and enable content/screen \
                        sharing in its settings, then try again (ttycast discover)."
                    .into(),
            );
        }
        let matched = renderers.iter().find(|renderer| {
            ctx.renderer_host
                .as_ref()
                .is_none_or(|host| renderer.host() == *host)
                && ctx
                    .renderer_name
                    .as_ref()
                    .is_none_or(|name| renderer.name.to_lowercase().contains(&name.to_lowercase()))
        });
        matched.cloned().ok_or_else(|| {
            let names: Vec<String> = renderers
                .iter()
                .map(|r| format!("{} ({})", r.name, r.host()))
                .collect();
            format!("no renderer matched. Found: {}", names.join(", "))
        })
    }
}

impl Backend for DlnaBackend {
    fn name(&self) -> &'static str {
        "dlna"
    }

    fn wants_ts(&self) -> bool {
        true
    }

    fn preflight(&self, ctx: &Context) -> Vec<String> {
        if !have_ffmpeg() {
            vec!["ffmpeg not found on PATH (needed to produce the H.264 stream)".into()]
        } else if pick_encoder(ctx.encoder.as_deref()).is_none() {
            vec![no_encoder_message()]
        } else {
            Vec::new()
        }
    }

    fn start(&mut self, ctx: &Context) -> Result<(), String> {
        let server = ctx
            .server
            .as_ref()
            .ok_or("the dlna backend needs the http server")?;
        let ts = ctx.ts.as_ref().ok_or("the dlna backend needs an encoder")?;
        let renderer = Self::choose(ctx)?;

        ts.start()?;
        let url = format!("{}/live.ts", server.base_url());
        ctx.log(&format!(
            "renderer: {} ({})",
            renderer.name,
            renderer.host()
        ));
        ctx.log(&format!("handing over: {url}"));
        upnp::play(&renderer, &url, "ttycast", "video/mpeg")
            .map_err(|e| format!("renderer refused the stream: {e}"))?;
        self.renderer = Some(renderer);
        Ok(())
    }

    fn on_frame(&mut self, ctx: &Context, _changed: bool) {
        // ffmpeg wants a frame every tick even when nothing moved, or the
        // stream's timestamps drift away from wall clock.
        if let (Some(ts), Some(server)) = (ctx.ts.as_ref(), ctx.server.as_ref())
            && let Some(frame) = server.bus().latest()
        {
            ts.write_frame(&frame);
        }
    }

    fn status(&self, ctx: &Context) -> String {
        match &self.renderer {
            None => "not connected".into(),
            Some(renderer) => format!(
                "{} <- {} conn",
                renderer.name,
                ctx.ts.as_ref().map_or(0, |ts| ts.client_count())
            ),
        }
    }

    fn stop(&mut self, _ctx: &Context) {
        if let Some(renderer) = self.renderer.take() {
            // TV already gone or switched input; nothing to do about it.
            let _ = upnp::stop(&renderer);
        }
    }
}
