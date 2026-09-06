//! Serve the terminal at a URL and let something open it.
//!
//! The lowest-common-denominator backend: no ffmpeg, no discovery, no codec
//! negotiation. MJPEG plays in every browser, phone and most smart-TV
//! browsers, and the latency is one frame.

use super::{Backend, Context};
use crate::server::local_ip;

pub struct BrowserBackend;

impl Backend for BrowserBackend {
    fn name(&self) -> &'static str {
        "browser"
    }

    fn start(&mut self, ctx: &Context) -> Result<(), String> {
        let base = ctx
            .server
            .as_ref()
            .ok_or("the browser backend needs the http server")?
            .base_url();
        ctx.log(&format!("open on the TV:  {base}/"));
        ctx.log(&format!("raw stream:      {base}/stream.mjpg"));
        Ok(())
    }

    fn status(&self, ctx: &Context) -> String {
        format!("http://{}:{}/", local_ip(), ctx.port)
    }
}
