//! Write frames to disk instead of a TV - for developing and for screenshots.

use std::path::PathBuf;

use super::{Backend, Context};
use crate::render::Frame;

#[derive(Default)]
pub struct PreviewBackend {
    path: PathBuf,
    written: u64,
}

/// Encode one frame as a PNG.
pub fn encode_png(frame: &Frame) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, frame.width as u32, frame.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&frame.data)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

impl Backend for PreviewBackend {
    fn name(&self) -> &'static str {
        "preview"
    }

    fn start(&mut self, ctx: &Context) -> Result<(), String> {
        self.path = PathBuf::from(&ctx.preview_path);
        ctx.log(&format!("writing frames to {}", self.path.display()));
        Ok(())
    }

    fn on_frame(&mut self, ctx: &Context, changed: bool) {
        if !changed {
            return;
        }
        let Some(server) = ctx.server.as_ref() else {
            return;
        };
        let Some(frame) = server.bus().latest() else {
            return;
        };
        let Ok(png) = encode_png(&frame) else { return };

        // Written beside the target and renamed, so a viewer never sees half a
        // file.
        let mut temporary = self.path.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        if std::fs::write(&temporary, png).is_ok()
            && std::fs::rename(&temporary, &self.path).is_ok()
        {
            self.written += 1;
        }
    }

    fn status(&self, _ctx: &Context) -> String {
        format!("{} ({} frames)", self.path.display(), self.written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_encoding_produces_a_real_file_header() {
        let frame = Frame {
            width: 4,
            height: 2,
            data: vec![128; 4 * 2 * 3],
        };
        let png = encode_png(&frame).unwrap();
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
    }
}
