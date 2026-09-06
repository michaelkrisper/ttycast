//! Fan-out point between the capture loop and whoever is watching.

use std::sync::{Arc, Condvar, Mutex};

use crate::render::Frame;

/// libjpeg-turbo through mozjpeg: fast where it does not show, exact where it
/// does.
///
/// Two settings decide this, and both were measured rather than assumed.
/// mozjpeg optimises for file size by default - trellis quantisation and
/// optimised Huffman tables - which costs 90 ms per 720p frame for bytes
/// nobody is counting on a LAN; turning that off is free quality-wise.
///
/// Chroma subsampling is the opposite trade. 4:2:0 halves colour resolution in
/// both axes, and a terminal is thin coloured glyphs on a dark background -
/// exactly the content it wrecks. Full 4:4:4 chroma costs 15.6 ms instead of
/// 8.7 ms per 1080p frame, which at 15 fps is affordable, and it is the
/// difference between crisp text and coloured fringes.
fn encode_jpeg(frame: &Frame, quality: u8) -> Option<Vec<u8>> {
    let mut compress = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
    compress.set_fastest_defaults();
    compress.set_optimize_coding(false);
    compress.set_optimize_scans(false);
    compress.set_chroma_sampling_pixel_sizes((1, 1), (1, 1));
    compress.set_size(frame.width, frame.height);
    compress.set_quality(f32::from(quality));
    let mut started = compress.start_compress(Vec::new()).ok()?;
    started.write_scanlines(&frame.data).ok()?;
    started.finish().ok()
}

#[derive(Default)]
struct State {
    frame: Option<Arc<Frame>>,
    jpeg: Option<Arc<Vec<u8>>>,
    seq: u64,
}

/// Holds the latest frame and wakes consumers when it changes.
///
/// Consumers block rather than poll, so an idle terminal costs nothing beyond
/// the capture loop itself. The JPEG is encoded lazily and cached, so a frame
/// nobody asks for is never compressed and ten clients share one encode.
pub struct FrameBus {
    state: Mutex<State>,
    changed: Condvar,
    quality: u8,
}

impl FrameBus {
    #[must_use]
    pub fn new(quality: u8) -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            quality,
        }
    }

    pub fn seq(&self) -> u64 {
        self.state.lock().unwrap().seq
    }

    pub fn publish(&self, frame: Frame) {
        let mut state = self.state.lock().unwrap();
        state.frame = Some(Arc::new(frame));
        state.jpeg = None;
        state.seq += 1;
        drop(state);
        self.changed.notify_all();
    }

    /// Wake every waiter so consumer threads can notice shutdown.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.seq += 1;
        drop(state);
        self.changed.notify_all();
    }

    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.state.lock().unwrap().frame.clone()
    }

    pub fn latest_jpeg(&self) -> Option<Arc<Vec<u8>>> {
        let mut state = self.state.lock().unwrap();
        if state.jpeg.is_none() {
            let frame = state.frame.clone()?;
            state.jpeg = Some(Arc::new(encode_jpeg(&frame, self.quality)?));
        }
        state.jpeg.clone()
    }

    /// Block until the sequence moves past `last`; return the new one.
    pub fn wait(&self, last: u64, timeout: std::time::Duration) -> u64 {
        let state = self.state.lock().unwrap();
        if state.seq != last {
            return state.seq;
        }
        let (state, _) = self.changed.wait_timeout(state, timeout).unwrap();
        state.seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn frame(shade: u8) -> Frame {
        Frame {
            width: 8,
            height: 8,
            data: vec![shade; 8 * 8 * 3],
        }
    }

    #[test]
    fn starts_empty() {
        let bus = FrameBus::new(80);
        assert_eq!(bus.seq(), 0);
        assert!(bus.latest().is_none());
        assert!(bus.latest_jpeg().is_none());
    }

    #[test]
    fn publish_advances_the_sequence() {
        let bus = FrameBus::new(80);
        bus.publish(frame(0));
        assert_eq!(bus.seq(), 1);
        assert!(bus.latest().is_some());
        bus.publish(frame(1));
        assert_eq!(bus.seq(), 2);
    }

    #[test]
    fn jpeg_is_encoded_once_per_frame() {
        let bus = FrameBus::new(80);
        bus.publish(frame(5));
        let first = bus.latest_jpeg().unwrap();
        assert_eq!(&first[..2], &[0xff, 0xd8]);
        assert!(Arc::ptr_eq(&first, &bus.latest_jpeg().unwrap()));
        bus.publish(frame(200));
        assert!(!Arc::ptr_eq(&first, &bus.latest_jpeg().unwrap()));
    }

    #[test]
    fn wait_returns_immediately_when_the_sequence_moved() {
        let bus = FrameBus::new(80);
        bus.publish(frame(0));
        assert_eq!(bus.wait(0, Duration::from_millis(50)), 1);
    }

    #[test]
    fn wait_blocks_until_a_frame_arrives() {
        let bus = Arc::new(FrameBus::new(80));
        let waiter = {
            let bus = Arc::clone(&bus);
            std::thread::spawn(move || bus.wait(0, Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(50));
        bus.publish(frame(0));
        assert_eq!(waiter.join().unwrap(), 1);
    }

    #[test]
    fn close_wakes_waiters() {
        let bus = Arc::new(FrameBus::new(80));
        let waiter = {
            let bus = Arc::clone(&bus);
            std::thread::spawn(move || bus.wait(0, Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(50));
        bus.close();
        assert_eq!(waiter.join().unwrap(), 1);
    }
}

#[cfg(test)]
mod quality_probe {
    use super::*;
    use crate::ansi::parse;
    use crate::fonts::FontSet;
    use crate::render::{Renderer, Theme};

    fn sample(width: usize, height: usize) -> Frame {
        let lines: Vec<String> = (0..30)
            .map(|i| {
                format!(
                    "\x1b[3{}m{:3}\x1b[0m \x1b[1msrc/main.rs\x1b[0m \x1b[2m{}\x1b[0m ok {}",
                    i % 8,
                    i,
                    "-".repeat(20),
                    i * 7
                )
            })
            .collect();
        let mut renderer = Renderer::new(
            width,
            height,
            Theme::default(),
            FontSet::load(None, None).unwrap(),
        );
        renderer.render(&parse(&lines.join("\n"), 100, 30))
    }

    fn encode(frame: &Frame, quality: u8, chroma: (u8, u8)) -> (f64, usize) {
        let mut best = f64::MAX;
        let mut size = 0;
        for _ in 0..8 {
            let start = std::time::Instant::now();
            let mut compress = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
            compress.set_fastest_defaults();
            compress.set_optimize_coding(false);
            compress.set_optimize_scans(false);
            compress.set_chroma_sampling_pixel_sizes(chroma, chroma);
            compress.set_size(frame.width, frame.height);
            compress.set_quality(f32::from(quality));
            let mut started = compress.start_compress(Vec::new()).unwrap();
            started.write_scanlines(&frame.data).unwrap();
            let out = started.finish().unwrap();
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
            size = out.len();
        }
        (best, size)
    }

    #[test]
    #[ignore = "reports numbers, asserts nothing"]
    fn quality_versus_time() {
        for (w, h) in [(1280usize, 720usize), (1920, 1080)] {
            let frame = sample(w, h);
            eprintln!("--- {w}x{h} ---");
            for chroma in [(2u8, 2u8), (1, 1)] {
                let label = if chroma == (2, 2) { "4:2:0" } else { "4:4:4" };
                for quality in [80u8, 85, 90, 95] {
                    let (ms, size) = encode(&frame, quality, chroma);
                    eprintln!(
                        "  {label}  q={quality}  {ms:5.2} ms  {:5.0} KiB",
                        size as f64 / 1024.0
                    );
                }
            }
        }
    }
}
