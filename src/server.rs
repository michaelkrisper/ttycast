//! The one HTTP endpoint every pull-based backend shares.
//!
//! Hand-rolled on `std::net`, one thread per connection: the whole surface is
//! five routes, and a framework would cost more than it saves.
//!
//! * `/`            a remote-friendly landing page
//! * `/stream.mjpg` MJPEG, lowest latency, plays in any browser
//! * `/frame.jpg`   the current frame as a single image
//! * `/live.ts`     H.264 in MPEG-TS, what a DLNA renderer is pointed at
//! * `/health`      plain-text status, handy from a second machine

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::encoder::TsBroadcaster;
use crate::framebus::FrameBus;

const BOUNDARY: &str = "ttycastframe";

/// DLNA renderers key off this; without it many refuse to start a live stream.
pub const DLNA_FEATURES_LIVE: &str =
    "DLNA.ORG_OP=00;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=8D500000000000000000000000000000";

fn page(host: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>ttycast</title>\n\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<style>\n\
         html,body{{margin:0;height:100%;background:#0c0c0e;color:#dedede;\
         font:16px/1.5 system-ui,sans-serif}}\n\
         body{{display:flex;flex-direction:column;align-items:center;justify-content:center}}\n\
         img{{max-width:100vw;max-height:100vh;object-fit:contain}}\n\
         a{{color:#ffb000}}\n\
         footer{{position:fixed;bottom:.5rem;opacity:.5;font-size:.8rem}}\n\
         </style></head><body>\n<img src=\"/stream.mjpg\" alt=\"terminal\">\n\
         <footer>ttycast &middot; <a href=\"/live.ts\">live.ts</a> &middot; {host}</footer>\n\
         </body></html>\n"
    )
}

/// The address a TV on the LAN would have to reach us on.
#[must_use]
pub fn local_ip() -> String {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return "127.0.0.1".into();
    };
    if socket.connect("8.8.8.8:53").is_err() {
        return "127.0.0.1".into();
    }
    socket
        .local_addr()
        .map_or_else(|_| "127.0.0.1".into(), |addr| addr.ip().to_string())
}

pub struct CastServer {
    bus: Arc<FrameBus>,
    ts: Option<Arc<TsBroadcaster>>,
    port: u16,
    viewers: AtomicUsize,
    shutting_down: AtomicBool,
    listener: Mutex<Option<TcpListener>>,
}

impl CastServer {
    pub fn bind(
        bus: Arc<FrameBus>,
        ts: Option<Arc<TsBroadcaster>>,
        host: &str,
        port: u16,
    ) -> Result<Arc<Self>, String> {
        let listener = TcpListener::bind((host, port))
            .map_err(|e| format!("cannot listen on {host}:{port}: {e}"))?;
        let port = listener.local_addr().map_or(port, |a| a.port());
        Ok(Arc::new(Self {
            bus,
            ts,
            port,
            viewers: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
            listener: Mutex::new(Some(listener)),
        }))
    }

    #[must_use]
    pub fn bus(&self) -> &Arc<FrameBus> {
        &self.bus
    }

    #[cfg(test)]
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", local_ip(), self.port)
    }

    /// How many clients are pulling a stream right now.
    ///
    /// This is what tells ttycast whether anybody is actually looking, and so
    /// whether the laptop's own screen is needed.
    pub fn viewers(&self) -> usize {
        self.viewers.load(Ordering::Relaxed)
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let listener = self
            .listener
            .lock()
            .unwrap()
            .take()
            .ok_or("server already started")?;
        let server = Arc::clone(self);
        std::thread::Builder::new()
            .name("ttycast-http".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if server.shutting_down.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let handler = Arc::clone(&server);
                    let _ = std::thread::Builder::new()
                        .name("ttycast-client".into())
                        .spawn(move || handler.handle(stream));
                }
            })
            .map_err(|e| format!("cannot start the http thread: {e}"))?;
        Ok(())
    }

    pub fn stop(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
        self.bus.close();
        // Unblock the accept loop by connecting to ourselves once.
        let _ = TcpStream::connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port));
    }

    fn handle(&self, mut stream: TcpStream) {
        let Some((method, path)) = read_request(&stream) else {
            return;
        };
        let path = path.split('?').next().unwrap_or("/").trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        let head_only = method == "HEAD";

        match path {
            "/" => send(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page(&local_ip()).as_bytes(),
                &[],
            ),
            "/health" => {
                let body = format!(
                    "ttycast ok\nframes={}\nviewers={}\nts_running={}\nts_clients={}\n",
                    self.bus.seq(),
                    self.viewers(),
                    self.ts.as_ref().is_some_and(|ts| ts.running()),
                    self.ts.as_ref().map_or(0, |ts| ts.client_count()),
                );
                send(
                    &mut stream,
                    200,
                    "text/plain; charset=utf-8",
                    body.as_bytes(),
                    &[],
                );
            }
            "/frame.jpg" => match self.bus.latest_jpeg() {
                Some(jpeg) => send(&mut stream, 200, "image/jpeg", &jpeg, &[]),
                None => send(&mut stream, 503, "text/plain", b"no frame yet\n", &[]),
            },
            "/stream.mjpg" => self.mjpeg(stream, head_only),
            "/live.ts" => self.live_ts(stream, head_only),
            _ => send(&mut stream, 404, "text/plain", b"no such stream\n", &[]),
        }
    }

    fn mjpeg(&self, mut stream: TcpStream, head_only: bool) {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary={BOUNDARY}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n"
        );
        if stream.write_all(header.as_bytes()).is_err() || head_only {
            return;
        }
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        self.viewers.fetch_add(1, Ordering::Relaxed);

        let mut seq = u64::MAX;
        while !self.shutting_down.load(Ordering::Relaxed) {
            seq = self.bus.wait(seq, Duration::from_secs(5));
            let Some(jpeg) = self.bus.latest_jpeg() else {
                continue;
            };
            let part = format!(
                "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                jpeg.len()
            );
            if stream.write_all(part.as_bytes()).is_err()
                || stream.write_all(&jpeg).is_err()
                || stream.write_all(b"\r\n").is_err()
            {
                break;
            }
        }
        self.viewers.fetch_sub(1, Ordering::Relaxed);
    }

    fn live_ts(&self, mut stream: TcpStream, head_only: bool) {
        let Some(ts) = self.ts.as_ref() else {
            send(
                &mut stream,
                503,
                "text/plain",
                b"transport stream not enabled\n",
                &[],
            );
            return;
        };
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: video/mpeg\r\nCache-Control: no-store\r\n\
             Accept-Ranges: none\r\nConnection: close\r\n\
             transferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: {DLNA_FEATURES_LIVE}\r\n\r\n"
        );
        if stream.write_all(header.as_bytes()).is_err() || head_only {
            return;
        }
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let receiver = ts.subscribe();
        self.viewers.fetch_add(1, Ordering::Relaxed);

        while !self.shutting_down.load(Ordering::Relaxed) {
            match receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(chunk) => {
                    if stream.write_all(&chunk).is_err() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        self.viewers.fetch_sub(1, Ordering::Relaxed);
    }
}

/// `(method, path)` from the request line, ignoring the headers we never use.
fn read_request(stream: &TcpStream) -> Option<(String, String)> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next().unwrap_or("/").to_string();
    // Drain the rest of the head so the client is not left writing into a
    // socket nobody reads.
    let mut header = String::new();
    while let Ok(read) = reader.read_line(&mut header) {
        if read == 0 || header.trim().is_empty() {
            break;
        }
        header.clear();
    }
    Some((method, path))
}

fn send(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    );
    for (key, value) in extra {
        let _ = write!(head, "{key}: {value}\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Frame;
    use std::io::Read;

    fn frame(shade: u8) -> Frame {
        Frame {
            width: 16,
            height: 16,
            data: vec![shade; 16 * 16 * 3],
        }
    }

    fn server() -> (Arc<CastServer>, Arc<FrameBus>) {
        let bus = Arc::new(FrameBus::new(80));
        let server = CastServer::bind(Arc::clone(&bus), None, "127.0.0.1", 0).unwrap();
        server.start().unwrap();
        (server, bus)
    }

    fn get(server: &CastServer, path: &str) -> (String, Vec<u8>) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", server.port())).expect("server should accept");
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = stream.read_to_end(&mut raw);
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a complete head");
        (
            String::from_utf8_lossy(&raw[..split]).into_owned(),
            raw[split + 4..].to_vec(),
        )
    }

    #[test]
    fn the_index_page_points_at_the_stream() {
        let (server, _bus) = server();
        let (head, body) = get(&server, "/");
        assert!(head.starts_with("HTTP/1.1 200 OK"));
        assert!(head.contains("text/html"));
        assert!(String::from_utf8_lossy(&body).contains("/stream.mjpg"));
        server.stop();
    }

    #[test]
    fn health_reports_frames_and_viewers() {
        let (server, bus) = server();
        bus.publish(frame(3));
        let (_, body) = get(&server, "/health");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("frames=1"), "{text}");
        assert!(text.contains("viewers=0"), "{text}");
        server.stop();
    }

    #[test]
    fn the_frame_endpoint_is_unavailable_before_the_first_frame() {
        let (server, _bus) = server();
        assert!(get(&server, "/frame.jpg").0.starts_with("HTTP/1.1 503"));
        server.stop();
    }

    #[test]
    fn the_frame_endpoint_serves_a_jpeg() {
        let (server, bus) = server();
        bus.publish(frame(200));
        let (head, body) = get(&server, "/frame.jpg");
        assert!(head.contains("image/jpeg"));
        assert_eq!(&body[..2], &[0xff, 0xd8]);
        server.stop();
    }

    #[test]
    fn unknown_paths_are_404() {
        let (server, _bus) = server();
        assert!(get(&server, "/nope").0.starts_with("HTTP/1.1 404"));
        server.stop();
    }

    #[test]
    fn live_ts_is_unavailable_without_an_encoder() {
        let (server, _bus) = server();
        assert!(get(&server, "/live.ts").0.starts_with("HTTP/1.1 503"));
        server.stop();
    }

    #[test]
    fn a_streaming_client_counts_as_a_viewer() {
        let (server, bus) = server();
        bus.publish(frame(10));
        let mut stream = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        write!(stream, "GET /stream.mjpg HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0u8; 512];
        let read = stream.read(&mut buffer).unwrap();
        assert!(String::from_utf8_lossy(&buffer[..read]).contains("multipart/x-mixed-replace"));

        let mut seen = 0;
        for _ in 0..50 {
            if server.viewers() == 1 {
                seen = 1;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(seen, 1, "the client should have been counted");
        server.stop();
    }

    #[test]
    fn the_dlna_headers_are_present_on_the_transport_stream() {
        // The header text is what a renderer inspects before it will play.
        assert!(DLNA_FEATURES_LIVE.contains("DLNA.ORG_OP=00"));
        assert!(DLNA_FEATURES_LIVE.contains("DLNA.ORG_FLAGS=8D50"));
    }

    #[test]
    fn local_ip_is_a_real_address() {
        let ip = local_ip();
        assert!(ip.parse::<IpAddr>().is_ok(), "{ip}");
    }
}
