//! What a backend is: a way of getting the stream in front of a TV.
//!
//! Capture, rendering and the HTTP endpoint are shared. A backend only decides
//! how the display learns about the stream - by being told a URL (browser), by
//! being handed one over UPnP (dlna), or by a wireless display session
//! (miracast).

pub mod browser;
pub mod dlna;
pub mod miracast;
pub mod preview;

use std::sync::Arc;

use crate::encoder::TsBroadcaster;
use crate::server::CastServer;

/// Everything a backend may need, filled in before `start`.
pub struct Context {
    pub width: usize,
    pub height: usize,
    pub fps: u32,
    pub bitrate: String,
    pub port: u16,
    pub encoder: Option<String>,
    pub server: Option<Arc<CastServer>>,
    pub ts: Option<Arc<TsBroadcaster>>,
    pub renderer_name: Option<String>,
    pub renderer_host: Option<String>,
    pub sink_host: Option<String>,
    pub sink_port: u16,
    pub preview_path: String,
    pub discovery_timeout: f64,
    pub quiet: bool,
}

impl Context {
    pub fn log(&self, message: &str) {
        if !self.quiet {
            eprintln!("  {message}");
        }
    }
}

pub trait Backend: Send {
    /// CLI name, unique.
    fn name(&self) -> &'static str;

    /// Whether the shared HTTP server should run an ffmpeg/MPEG-TS pipeline.
    fn wants_ts(&self) -> bool {
        false
    }

    /// Blocking problems; empty means good to go.
    fn preflight(&self, _ctx: &Context) -> Vec<String> {
        Vec::new()
    }

    /// Called once the server is up and the first frame exists.
    fn start(&mut self, _ctx: &Context) -> Result<(), String> {
        Ok(())
    }

    /// Called for every tick of the capture loop.
    fn on_frame(&mut self, _ctx: &Context, _changed: bool) {}

    /// Short line for the terminal status bar.
    fn status(&self, _ctx: &Context) -> String {
        String::new()
    }

    /// Always called, including after a failed `start`.
    fn stop(&mut self, _ctx: &Context) {}
}

/// `(name, one-line summary)` for `--help` and `ttycast backends`.
pub const SUMMARIES: [(&str, &str); 4] = [
    (
        "browser",
        "serve MJPEG at a URL; open it on the TV, a phone or a laptop",
    ),
    (
        "dlna",
        "find a DLNA renderer and hand it the stream (works on most smart TVs)",
    ),
    (
        "miracast",
        "wireless display over Wi-Fi Direct (lowest latency; needs P2P hardware)",
    ),
    (
        "preview",
        "write the rendered frame to a PNG file (no network, for testing)",
    ),
];

pub fn create(name: &str) -> Result<Box<dyn Backend>, String> {
    match name {
        "browser" => Ok(Box::new(browser::BrowserBackend)),
        "dlna" => Ok(Box::new(dlna::DlnaBackend::default())),
        "miracast" => Ok(Box::new(miracast::MiracastBackend::default())),
        "preview" => Ok(Box::new(preview::PreviewBackend::default())),
        other => Err(format!(
            "unknown backend {other:?} (known: {})",
            SUMMARIES.map(|(n, _)| n).join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            width: 1280,
            height: 720,
            fps: 10,
            bitrate: "2M".into(),
            port: 8009,
            encoder: None,
            server: None,
            ts: None,
            renderer_name: None,
            renderer_host: None,
            sink_host: None,
            sink_port: 5000,
            preview_path: "preview.png".into(),
            discovery_timeout: 3.0,
            quiet: true,
        }
    }

    #[test]
    fn every_summarised_backend_can_be_created() {
        for (name, summary) in SUMMARIES {
            assert!(!summary.is_empty(), "{name} has no summary");
            assert_eq!(create(name).unwrap().name(), name);
        }
    }

    #[test]
    fn unknown_backend_names_are_rejected() {
        let Err(error) = create("beamer") else {
            panic!("an unknown backend must be rejected");
        };
        assert!(error.contains("beamer"), "{error}");
        assert!(error.contains("browser"), "{error}");
    }

    #[test]
    fn the_browser_backend_needs_nothing() {
        assert!(create("browser").unwrap().preflight(&context()).is_empty());
    }

    #[test]
    fn only_the_dlna_backend_wants_a_transport_stream() {
        assert!(create("dlna").unwrap().wants_ts());
        assert!(!create("browser").unwrap().wants_ts());
        assert!(!create("preview").unwrap().wants_ts());
        assert!(!create("miracast").unwrap().wants_ts());
    }

    #[test]
    fn miracast_preflight_skips_the_radio_check_for_a_known_sink() {
        let mut ctx = context();
        ctx.sink_host = Some("192.168.49.1".into());
        let problems = create("miracast").unwrap().preflight(&ctx);
        assert!(
            !problems.iter().any(|p| p.contains("Wi-Fi Direct")),
            "{problems:?}"
        );
    }

    #[test]
    fn miracast_start_explains_what_is_missing_without_a_sink() {
        let ctx = context();
        let error = create("miracast").unwrap().start(&ctx).unwrap_err();
        assert!(error.contains("--sink-host"), "{error}");
    }
}
