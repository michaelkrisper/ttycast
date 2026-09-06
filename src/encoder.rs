//! H.264/MPEG-TS encoding through ffmpeg, plus fan-out of the muxed bytes.
//!
//! Frames go in as raw RGB on stdin and come out as an endless transport
//! stream, which is what a DLNA renderer pulls over HTTP and what Miracast
//! carries over RTP.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};

use crate::capture::which;
use crate::render::Frame;

/// H.264 encoders ttycast knows how to drive, best first.
///
/// Distributions that ship a patent-free ffmpeg (Fedora's `ffmpeg-free`) have
/// no libx264, but usually do have libopenh264 or a VAAPI encoder on the GPU.
pub const H264_ENCODERS: [&str; 5] = [
    "libx264",
    "libopenh264",
    "h264_vaapi",
    "h264_qsv",
    "h264_nvenc",
];

const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

#[must_use]
pub fn have_ffmpeg() -> bool {
    which("ffmpeg").is_some()
}

fn available_encoders() -> &'static HashSet<String> {
    static ENCODERS: OnceLock<HashSet<String>> = OnceLock::new();
    ENCODERS.get_or_init(|| {
        let Ok(out) = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
        else {
            return HashSet::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| line.starts_with(" V"))
            .filter_map(|line| line.split_whitespace().nth(1))
            .map(ToString::to_string)
            .collect()
    })
}

/// The best H.264 encoder present, or `None` if there is none.
#[must_use]
pub fn pick_encoder(preferred: Option<&str>) -> Option<String> {
    let have = available_encoders();
    if let Some(name) = preferred {
        return have.contains(name).then(|| name.to_string());
    }
    H264_ENCODERS
        .iter()
        .find(|name| have.contains(**name))
        .map(|name| (*name).to_string())
}

#[must_use]
pub fn no_encoder_message() -> String {
    format!(
        "ffmpeg has no usable H.264 encoder (looked for {}). On Fedora, ffmpeg-free \
         ships without libx264: install openh264/libopenh264 or use the GPU encoder h264_vaapi.",
        H264_ENCODERS.join(", ")
    )
}

fn input_args(width: usize, height: usize, fps: u32, encoder: &str) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error"]
        .iter()
        .map(ToString::to_string)
        .collect();
    if encoder == "h264_vaapi" {
        // The device has to be opened before the input that will use it.
        args.extend(["-vaapi_device".into(), VAAPI_DEVICE.to_string()]);
    }
    args.extend(
        [
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            &format!("{width}x{height}"),
            "-r",
            &fps.to_string(),
            "-i",
            "pipe:0",
            "-an",
        ]
        .iter()
        .map(ToString::to_string),
    );
    args
}

/// H.264 tuned for a still picture that changes in bursts.
///
/// The GOP is deliberately short: a receiver that joins mid-stream shows
/// nothing until the next keyframe, and a terminal is cheap to re-send.
fn video_args(
    encoder: &str,
    fps: u32,
    bitrate: &str,
    gop_seconds: u32,
    preset: &str,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if encoder == "h264_vaapi" {
        args.extend(["-vf".into(), "format=nv12,hwupload".to_string()]);
    } else {
        args.extend(["-pix_fmt".into(), "yuv420p".to_string()]);
    }
    args.extend(["-c:v".into(), encoder.to_string()]);
    if encoder == "libx264" {
        // Only libx264 takes these; another encoder errors out on them.
        args.extend(
            [
                "-preset",
                preset,
                "-tune",
                "zerolatency",
                "-profile:v",
                "high",
            ]
            .iter()
            .map(ToString::to_string),
        );
    }
    args.extend(
        [
            "-b:v",
            bitrate,
            "-maxrate",
            bitrate,
            "-bufsize",
            bitrate,
            "-g",
            &(fps * gop_seconds).max(1).to_string(),
        ]
        .iter()
        .map(ToString::to_string),
    );
    args
}

/// Raw RGB on stdin to MPEG-TS on stdout - what a DLNA renderer pulls.
#[must_use]
pub fn ts_command(
    width: usize,
    height: usize,
    fps: u32,
    bitrate: &str,
    encoder: &str,
) -> Vec<String> {
    let mut args = input_args(width, height, fps, encoder);
    args.extend(video_args(encoder, fps, bitrate, 2, "veryfast"));
    args.extend(
        [
            "-f",
            "mpegts",
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
            "pipe:1",
        ]
        .iter()
        .map(ToString::to_string),
    );
    args
}

/// Raw RGB on stdin to RTP/MPEG-TS on the wire - what Miracast carries.
#[must_use]
pub fn rtp_command(
    width: usize,
    height: usize,
    fps: u32,
    host: &str,
    port: u16,
    bitrate: &str,
    encoder: &str,
) -> Vec<String> {
    let mut args = input_args(width, height, fps, encoder);
    args.extend(video_args(encoder, fps, bitrate, 1, "ultrafast"));
    args.extend(
        [
            "-f".to_string(),
            "rtp_mpegts".to_string(),
            // 1316 keeps an RTP packet under the wifi MTU.
            format!("rtp://{host}:{port}?pkt_size=1316"),
        ]
        .iter()
        .map(ToString::to_string),
    );
    args
}

fn spawn(args: &[String], capture_stdout: bool) -> Result<Child, String> {
    if !have_ffmpeg() {
        return Err("ffmpeg not found on PATH".into());
    }
    Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg: {e}"))
}

/// One ffmpeg process, many HTTP clients.
///
/// Each client gets a bounded channel; one that cannot keep up is dropped
/// rather than allowed to stall the encoder.
pub struct TsBroadcaster {
    args: Vec<String>,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    clients: Mutex<Vec<SyncSender<Arc<Vec<u8>>>>>,
}

impl TsBroadcaster {
    #[must_use]
    pub fn new(width: usize, height: usize, fps: u32, bitrate: &str, encoder: &str) -> Self {
        Self {
            args: ts_command(width, height, fps, bitrate, encoder),
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            clients: Mutex::new(Vec::new()),
        }
    }

    pub fn running(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let mut slot = self.child.lock().unwrap();
        if slot.is_some() {
            return Ok(());
        }
        let mut child = spawn(&self.args, true)?;
        *self.stdin.lock().unwrap() = child.stdin.take();
        let stdout = child.stdout.take().ok_or("ffmpeg gave no stdout")?;
        *slot = Some(child);
        drop(slot);

        let bus = Arc::clone(self);
        std::thread::Builder::new()
            .name("ttycast-ts".into())
            .spawn(move || bus.drain(stdout))
            .map_err(|e| format!("cannot start the muxer thread: {e}"))?;
        Ok(())
    }

    fn drain(&self, mut stdout: std::process::ChildStdout) {
        let mut buffer = vec![0u8; 16 * 1024];
        while let Ok(read) = stdout.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let chunk = Arc::new(buffer[..read].to_vec());
            self.clients
                .lock()
                .unwrap()
                .retain(|client| client.try_send(Arc::clone(&chunk)).is_ok());
        }
    }

    /// Push one frame; `false` once ffmpeg is gone.
    pub fn write_frame(&self, frame: &Frame) -> bool {
        let mut slot = self.stdin.lock().unwrap();
        let Some(stdin) = slot.as_mut() else {
            return false;
        };
        stdin.write_all(&frame.data).is_ok()
    }

    pub fn subscribe(&self) -> Receiver<Arc<Vec<u8>>> {
        let (sender, receiver) = sync_channel(64);
        self.clients.lock().unwrap().push(sender);
        receiver
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().unwrap().len()
    }

    pub fn stop(&self) {
        self.stdin.lock().unwrap().take();
        self.clients.lock().unwrap().clear();
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Push the stream straight at a sink over RTP - the Miracast transport.
pub struct RtpSender {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    args: Vec<String>,
}

impl RtpSender {
    #[must_use]
    pub fn new(
        width: usize,
        height: usize,
        fps: u32,
        host: &str,
        port: u16,
        bitrate: &str,
        encoder: &str,
    ) -> Self {
        Self {
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            args: rtp_command(width, height, fps, host, port, bitrate, encoder),
        }
    }

    pub fn start(&self) -> Result<(), String> {
        let mut slot = self.child.lock().unwrap();
        if slot.is_some() {
            return Ok(());
        }
        let mut child = spawn(&self.args, false)?;
        *self.stdin.lock().unwrap() = child.stdin.take();
        *slot = Some(child);
        Ok(())
    }

    pub fn write_frame(&self, frame: &Frame) -> bool {
        let mut slot = self.stdin.lock().unwrap();
        let Some(stdin) = slot.as_mut() else {
            return false;
        };
        stdin.write_all(&frame.data).is_ok()
    }

    pub fn stop(&self) {
        self.stdin.lock().unwrap().take();
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(args: &[String], flag: &str) -> usize {
        args.iter().position(|a| a == flag).expect(flag)
    }

    #[test]
    fn ts_command_declares_the_raw_input() {
        let cmd = ts_command(1280, 720, 5, "2M", "libx264");
        assert!(cmd.contains(&"rawvideo".to_string()));
        assert!(cmd.contains(&"1280x720".to_string()));
        assert_eq!(cmd[position(&cmd, "-r") + 1], "5");
        assert_eq!(cmd.last().unwrap(), "pipe:1");
    }

    #[test]
    fn ts_command_muxes_to_mpegts() {
        let cmd = ts_command(640, 480, 10, "2M", "libx264");
        assert!(cmd.windows(2).any(|w| w[0] == "-f" && w[1] == "mpegts"));
    }

    #[test]
    fn gop_follows_the_frame_rate() {
        let cmd = ts_command(640, 480, 5, "2M", "libx264");
        assert_eq!(cmd[position(&cmd, "-g") + 1], "10");
    }

    #[test]
    fn bitrate_is_capped_not_just_targeted() {
        let cmd = ts_command(640, 480, 5, "3M", "libx264");
        for flag in ["-b:v", "-maxrate", "-bufsize"] {
            assert_eq!(cmd[position(&cmd, flag) + 1], "3M");
        }
    }

    #[test]
    fn libx264_only_options_stay_with_libx264() {
        assert!(ts_command(640, 480, 5, "2M", "libx264").contains(&"-tune".to_string()));
        assert!(!ts_command(640, 480, 5, "2M", "libopenh264").contains(&"-tune".to_string()));
    }

    #[test]
    fn vaapi_opens_the_device_before_the_input() {
        let cmd = ts_command(640, 480, 5, "2M", "h264_vaapi");
        assert!(position(&cmd, "-vaapi_device") < position(&cmd, "-i"));
        assert!(cmd.contains(&"format=nv12,hwupload".to_string()));
    }

    #[test]
    fn rtp_command_targets_the_sink() {
        let cmd = rtp_command(1280, 720, 30, "192.168.49.1", 5000, "6M", "libx264");
        assert_eq!(cmd[cmd.len() - 2], "rtp_mpegts");
        let url = cmd.last().unwrap();
        assert!(url.starts_with("rtp://192.168.49.1:5000"));
        assert!(url.contains("pkt_size=1316"));
    }

    #[test]
    fn a_broadcaster_starts_idle() {
        let ts = TsBroadcaster::new(64, 64, 5, "1M", "libx264");
        assert!(!ts.running());
        assert_eq!(ts.client_count(), 0);
    }

    #[test]
    fn subscribers_are_tracked() {
        let ts = TsBroadcaster::new(64, 64, 5, "1M", "libx264");
        let _receiver = ts.subscribe();
        assert_eq!(ts.client_count(), 1);
    }

    #[test]
    fn stopping_without_starting_is_harmless() {
        TsBroadcaster::new(64, 64, 5, "1M", "libx264").stop();
        RtpSender::new(64, 64, 5, "127.0.0.1", 5000, "1M", "libx264").stop();
    }

    #[test]
    fn pick_encoder_rejects_one_that_is_not_there() {
        assert_eq!(pick_encoder(Some("definitely_not_an_encoder")), None);
    }

    #[test]
    fn the_no_encoder_message_names_what_was_tried() {
        let message = no_encoder_message();
        for name in H264_ENCODERS {
            assert!(message.contains(name), "{name} missing from the advice");
        }
    }
}
