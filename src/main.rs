//! Command line: start with the shell, stop with it.
//!
//! The server has no daemon and no state on disk. It comes up when `ttycast`
//! runs, it goes away when `ttycast` exits or is interrupted, and the TV is
//! told to stop on the way out.

mod ansi;
mod backend;
mod capture;
mod display;
mod doctor;
mod encoder;
mod fonts;
mod framebus;
mod render;
mod server;
mod upnp;
mod wfd;
mod wifi;

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

use crate::backend::Context;
use crate::capture::PaneSource;
use crate::encoder::TsBroadcaster;
use crate::framebus::FrameBus;
use crate::render::{Renderer, Theme};
use crate::server::CastServer;

const DEFAULT_PORT: u16 = 8009;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Ask the kernel to flip a flag instead of killing us, so the screen gets
/// restored and the TV gets told to stop.
fn install_signal_handlers() {
    // SAFETY: the handler only stores into a static atomic, which is
    // async-signal-safe.
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
        // A client vanishing mid-write must not take the process with it.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

fn parse_size(text: &str) -> Result<(usize, usize), String> {
    let (width, height) = text
        .to_lowercase()
        .split_once('x')
        .map(|(w, h)| (w.to_string(), h.to_string()))
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, got {text:?}"))?;
    Ok((
        width
            .parse()
            .map_err(|_| format!("bad width in {text:?}"))?,
        height
            .parse()
            .map_err(|_| format!("bad height in {text:?}"))?,
    ))
}

#[derive(Parser, Debug)]
#[command(
    name = "ttycast",
    version,
    about = "Cast a terminal to a TV.",
    after_help = "backends:\n  \
        browser   serve MJPEG at a URL; open it on the TV, a phone or a laptop\n  \
        dlna      find a DLNA renderer and hand it the stream (works on most smart TVs)\n  \
        miracast  wireless display over Wi-Fi Direct (lowest latency; needs P2P hardware)\n  \
        preview   write the rendered frame to a PNG file (no network, for testing)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start casting (the default).
    Run(Box<RunArgs>),
    /// Check what works on this machine.
    Doctor {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Skip the LAN scan.
        #[arg(long)]
        no_discover: bool,
    },
    /// List DLNA renderers on the LAN.
    Discover {
        #[arg(long, default_value_t = 3.0)]
        timeout: f64,
    },
    /// List available backends.
    Backends,
    /// Undo --screen-off after a crash.
    ScreenOn,
}

#[derive(clap::Args, Debug, Clone)]
struct RunArgs {
    /// How to reach the TV.
    #[arg(short, long, default_value = "browser",
          value_parser = ["browser", "dlna", "miracast", "preview"])]
    backend: String,

    /// tmux target: auto (active pane), self (this pane), new (own session),
    /// or any tmux target string.
    #[arg(short, long, default_value = "auto")]
    target: String,

    /// Capture rate.
    #[arg(short, long, default_value_t = 10)]
    fps: u32,

    /// Output resolution.
    #[arg(short, long, default_value = "1280x720", value_parser = parse_size)]
    size: (usize, usize),

    /// H.264 bitrate for dlna/miracast.
    #[arg(long, default_value = "2M")]
    bitrate: String,

    /// Force an ffmpeg H.264 encoder (default: autodetect).
    #[arg(long)]
    encoder: Option<String>,

    /// HTTP port to serve on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Path to a monospace font file.
    #[arg(long)]
    font: Option<String>,

    /// JPEG quality for MJPEG.
    #[arg(long, default_value_t = 80)]
    quality: u8,

    /// dlna: pick a renderer by name substring.
    #[arg(long)]
    renderer: Option<String>,

    /// dlna: pick a renderer by IP.
    #[arg(long)]
    renderer_host: Option<String>,

    /// miracast: IP of an already connected sink.
    #[arg(long)]
    sink_host: Option<String>,

    /// miracast: RTP port.
    #[arg(long, default_value_t = 5000)]
    sink_port: u16,

    /// preview: file to write.
    #[arg(long, default_value = "ttycast-preview.png")]
    preview_path: String,

    /// Darken this laptop's own screen while someone is watching the stream.
    #[arg(long, num_args = 0..=1, default_missing_value = "auto",
          value_parser = ["auto", "sway", "wlopm", "xset", "backlight"])]
    screen_off: Option<String>,

    /// Render a single frame and exit.
    #[arg(long)]
    once: bool,

    /// No status line.
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Doctor { port, no_discover }) => command_doctor(port, !no_discover),
        Some(Command::Discover { timeout }) => command_discover(timeout),
        Some(Command::Backends) => command_backends(),
        Some(Command::ScreenOn) => command_screen_on(),
        Some(Command::Run(args)) => command_run(*args),
        None => command_run(cli.run),
    };
    std::process::ExitCode::from(code)
}

fn command_backends() -> u8 {
    for (name, summary) in backend::SUMMARIES {
        println!("{name:<9} {summary}");
    }
    0
}

fn command_screen_on() -> u8 {
    let restored = display::restore_all();
    if restored.is_empty() {
        println!("nothing to restore");
    } else {
        println!("restored: {}", restored.join(", "));
    }
    0
}

fn command_discover(timeout: f64) -> u8 {
    let renderers = upnp::discover(Duration::from_secs_f64(timeout));
    if renderers.is_empty() {
        eprintln!(
            "no DLNA renderer answered.\nMost TVs only announce one while content/screen \
             sharing is enabled in their settings, and never while they are off."
        );
        return 1;
    }
    for renderer in renderers {
        println!(
            "{}\t{}\t{}",
            renderer.name,
            renderer.host(),
            renderer.control_url
        );
    }
    0
}

fn command_doctor(port: u16, discover: bool) -> u8 {
    let checks = doctor::run(port, discover);
    println!("{}", doctor::format(&checks));
    println!("\nthis machine is reachable at {}", server::local_ip());
    u8::from(!checks.iter().all(|c| c.ok || c.optional))
}

/// Owns everything that has to be torn down again.
struct Session {
    args: RunArgs,
    backend: Box<dyn backend::Backend>,
    ctx: Context,
    renderer: Renderer,
    screen: display::ScreenPower,
    frames: u64,
    tick_ms: f64,
    last_capture: Option<(String, usize, usize, (usize, usize))>,
}

impl Session {
    fn new(args: RunArgs) -> Result<Self, String> {
        let backend = backend::create(&args.backend)?;
        let (width, height) = args.size;
        let (regular, bold) = fonts::load(args.font.as_deref(), None)?;
        let screen = display::ScreenPower::new(
            args.screen_off
                .as_ref()
                .and_then(|pref| display::pick(pref)),
        );
        let ctx = Context {
            width,
            height,
            fps: args.fps.max(1),
            bitrate: args.bitrate.clone(),
            port: args.port,
            encoder: args.encoder.clone(),
            server: None,
            ts: None,
            renderer_name: args.renderer.clone(),
            renderer_host: args.renderer_host.clone(),
            sink_host: args.sink_host.clone(),
            sink_port: args.sink_port,
            preview_path: args.preview_path.clone(),
            discovery_timeout: 3.0,
            quiet: args.quiet,
        };
        Ok(Self {
            backend,
            renderer: Renderer::new(width, height, Theme::default(), regular, bold),
            screen,
            ctx,
            args,
            frames: 0,
            tick_ms: 0.0,
            last_capture: None,
        })
    }

    fn log(&self, message: &str) {
        if !self.args.quiet {
            eprintln!("  {message}");
        }
    }

    fn tick(&mut self, source: &PaneSource) -> bool {
        let (info, text) = match source.read_raw() {
            Ok(found) => found,
            Err(error) => {
                let frame = self
                    .renderer
                    .message("ttycast", &["no pane to mirror".into(), error]);
                if let Some(server) = self.ctx.server.as_ref() {
                    server.bus().publish(frame);
                }
                self.last_capture = None;
                return true;
            }
        };
        // Comparing the raw capture is a string compare; it costs microseconds
        // and saves the parse and the render on every idle tick.
        let signature = (text.clone(), info.width, info.height, info.cursor);
        if self.last_capture.as_ref() == Some(&signature) {
            return false;
        }
        self.last_capture = Some(signature);

        let mut screen = ansi::parse(&text, info.width, info.height);
        screen.cursor = Some(info.cursor);
        let frame = self.renderer.render(&screen);
        if let Some(server) = self.ctx.server.as_ref() {
            server.bus().publish(frame);
        }
        self.frames += 1;
        true
    }

    fn status(&self) {
        if self.args.quiet {
            return;
        }
        let cost = if self.tick_ms > 0.0 {
            format!("{:4.1} ms/frame", self.tick_ms)
        } else {
            String::new()
        };
        let screen = if self.screen.active() {
            format!("  [{}]", self.screen.status())
        } else {
            String::new()
        };
        let line = format!(
            "\r  {} frames  {cost}  {}{screen}  (ctrl-c to stop)",
            self.frames,
            self.backend.status(&self.ctx)
        );
        let line: String = line.chars().take(110).collect();
        eprint!("{line:<110}");
        let _ = std::io::stderr().flush();
    }

    fn run(&mut self) -> Result<u8, String> {
        let problems = self.backend.preflight(&self.ctx);
        if !problems.is_empty() {
            eprintln!("ttycast: backend '{}' cannot start:", self.backend.name());
            for problem in problems {
                eprintln!("  - {problem}");
            }
            return Ok(2);
        }

        let source = PaneSource::new(capture::resolve_target(&self.args.target)?);
        let bus = Arc::new(FrameBus::new(self.args.quality));
        let ts = if self.backend.wants_ts() {
            let encoder = encoder::pick_encoder(self.ctx.encoder.as_deref())
                .ok_or_else(encoder::no_encoder_message)?;
            Some(Arc::new(TsBroadcaster::new(
                self.ctx.width,
                self.ctx.height,
                self.ctx.fps,
                &self.ctx.bitrate,
                &encoder,
            )))
        } else {
            None
        };
        let server = CastServer::bind(bus, ts.clone(), "0.0.0.0", self.args.port)
            .map_err(|e| format!("{e}. Pick another with --port."))?;
        server.start()?;
        self.ctx.server = Some(Arc::clone(&server));
        self.ctx.ts = ts;

        if !self.args.quiet {
            eprintln!(
                "ttycast {}  {}x{} @ {} fps  backend={}",
                env!("CARGO_PKG_VERSION"),
                self.ctx.width,
                self.ctx.height,
                self.ctx.fps,
                self.backend.name()
            );
        }
        if self.args.screen_off.is_some() {
            if let Some(method) = self.screen.method {
                self.log(&format!("screen off while watched: {}", method.describes()));
                self.log("if ttycast dies without restoring it: ttycast screen-on");
            } else {
                self.log("no way to darken the screen here");
            }
        }

        // One frame before the backend starts, so a TV that connects at once
        // never sees an empty stream.
        self.tick(&source);

        if let Err(error) = self.backend.start(&self.ctx) {
            eprintln!("ttycast: {error}");
            return Ok(2);
        }

        if self.args.once {
            self.backend.on_frame(&self.ctx, true);
            return Ok(0);
        }

        let interval = Duration::from_secs_f64(1.0 / f64::from(self.ctx.fps));
        let started = Instant::now();
        let mut next = Instant::now();
        while !STOP.load(Ordering::SeqCst) {
            let began = Instant::now();
            let changed = self.tick(&source);
            self.backend.on_frame(&self.ctx, changed);
            if changed {
                self.tick_ms = began.elapsed().as_secs_f64() * 1000.0;
            }
            self.screen
                .update(server.viewers(), started.elapsed().as_secs_f64());

            next += interval;
            match next.checked_duration_since(Instant::now()) {
                Some(delay) => std::thread::sleep(delay),
                // Fell behind; do not spiral trying to catch up.
                None => next = Instant::now(),
            }
            self.status();
        }
        Ok(0)
    }

    fn shutdown(&mut self) {
        // First thing on the way out: the user needs their screen back.
        self.screen.restore();
        self.backend.stop(&self.ctx);
        if let Some(ts) = self.ctx.ts.as_ref() {
            ts.stop();
        }
        if let Some(server) = self.ctx.server.as_ref() {
            server.stop();
        }
        if !self.args.quiet {
            eprintln!();
        }
    }
}

fn command_run(args: RunArgs) -> u8 {
    install_signal_handlers();
    let mut session = match Session::new(args) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("ttycast: {error}");
            return 2;
        }
    };
    let code = match session.run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ttycast: {error}");
            2
        }
    };
    session.shutdown();
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_size_accepts_both_cases() {
        assert_eq!(parse_size("1920x1080").unwrap(), (1920, 1080));
        assert_eq!(parse_size("640X480").unwrap(), (640, 480));
        assert!(parse_size("big").is_err());
        assert!(parse_size("100xtall").is_err());
    }

    #[test]
    fn defaults_are_smooth_but_cheap() {
        let cli = Cli::parse_from(["ttycast"]);
        assert_eq!(cli.run.fps, 10);
        assert_eq!(cli.run.size, (1280, 720));
        assert_eq!(cli.run.backend, "browser");
        assert_eq!(cli.run.port, DEFAULT_PORT);
        assert!(cli.run.screen_off.is_none());
    }

    #[test]
    fn a_backend_can_be_selected_without_a_subcommand() {
        let cli = Cli::parse_from(["ttycast", "--backend", "dlna", "--fps", "30"]);
        assert_eq!(cli.run.backend, "dlna");
        assert_eq!(cli.run.fps, 30);
    }

    #[test]
    fn screen_off_defaults_to_auto_when_given_no_value() {
        let cli = Cli::parse_from(["ttycast", "--screen-off"]);
        assert_eq!(cli.run.screen_off.as_deref(), Some("auto"));
        let cli = Cli::parse_from(["ttycast", "--screen-off", "backlight"]);
        assert_eq!(cli.run.screen_off.as_deref(), Some("backlight"));
    }

    #[test]
    fn an_unknown_backend_is_rejected_by_the_parser() {
        assert!(Cli::try_parse_from(["ttycast", "--backend", "beamer"]).is_err());
    }

    #[test]
    fn subcommands_parse() {
        assert!(matches!(
            Cli::parse_from(["ttycast", "doctor", "--no-discover"]).command,
            Some(Command::Doctor {
                no_discover: true,
                ..
            })
        ));
        assert!(matches!(
            Cli::parse_from(["ttycast", "screen-on"]).command,
            Some(Command::ScreenOn)
        ));
        assert!(matches!(
            Cli::parse_from(["ttycast", "backends"]).command,
            Some(Command::Backends)
        ));
    }

    #[test]
    fn backends_are_listed_with_a_summary() {
        assert_eq!(command_backends(), 0);
        assert_eq!(backend::SUMMARIES.len(), 4);
    }
}
