//! uadl — download an Unacademy class as a single MP4 with the whiteboard
//! rendered in.
//!
//! A class replay is not one video. `output.webm` is only the educator's
//! webcam and audio; every slide and pen stroke lives in `data.json` and is
//! drawn client-side. Downloading "the video" gets you a talking head with no
//! board. This reconstructs the board and composites the two.

mod api;
mod assets;
mod download;
mod export;
mod model;
mod render;
mod state;
mod yuv;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Result};
use clap::Parser;

use assets::SlideCache;

#[derive(Parser, Debug)]
#[command(
    name = "uadl",
    about = "Download an Unacademy class as a single MP4 with the whiteboard rendered in",
    after_help = "example:\n  \
        uadl https://unacademy.com/class/slug/ABCD1234 eyJhbGci...\n  \
        uadl ABCD1234 \"$UNACADEMY_TOKEN\" -o ~/classes --fast"
)]
struct Args {
    /// Class URL or bare class uid
    url: String,

    /// accessToken cookie value (or set UNACADEMY_TOKEN)
    token: Option<String>,

    /// Output directory
    #[arg(short, long, default_value = ".")]
    out: PathBuf,

    /// Download connections
    #[arg(short = 'c', long, default_value_t = 16)]
    connections: usize,

    /// Render threads (default: cores)
    #[arg(short = 'j', long, default_value_t = 0)]
    jobs: usize,

    /// Output frame rate
    #[arg(long, default_value_t = 12.0)]
    fps: f64,

    /// Board width in pixels
    #[arg(long, default_value_t = 1280)]
    width: u32,

    /// x264 quality, lower is better
    #[arg(long, default_value_t = 28)]
    crf: u32,

    /// x264 preset
    #[arg(long, default_value = "superfast")]
    preset: String,

    /// Fewer pixels and frames: 960px, 8fps, crf 30
    #[arg(long, conflicts_with = "best")]
    fast: bool,

    /// Smaller and sharper, slower: veryfast, crf 23
    #[arg(long)]
    best: bool,

    /// Drop the webcam inset
    #[arg(long)]
    no_pip: bool,

    /// Ignore the educator's zoom and pan
    #[arg(long)]
    no_zoom: bool,

    /// Keep the downloaded webm and event JSON
    #[arg(long)]
    keep: bool,

    /// Start at this many seconds
    #[arg(long, default_value_t = 0.0)]
    start: f64,

    /// Stop at this many seconds
    #[arg(long)]
    end: Option<f64>,

    #[arg(short, long)]
    quiet: bool,
}

/// One-line progress that refreshes in place, when stderr is a terminal.
struct Reporter {
    tty: bool,
}

impl Reporter {
    fn new(quiet: bool) -> Self {
        Reporter {
            tty: !quiet && std::io::stderr().is_terminal(),
        }
    }

    fn say(&self, msg: &str) {
        eprintln!("{msg}");
    }

    fn line(&self, msg: &str) {
        if self.tty {
            eprint!("\r{msg}    ");
            let _ = std::io::stderr().flush();
        }
    }

    fn done(&self, msg: &str) {
        if self.tty {
            eprint!("\r{msg}    \n");
            let _ = std::io::stderr().flush();
        } else {
            eprintln!("{msg}");
        }
    }
}

/// Format a float without a pointless trailing `.0`.
fn trim_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v}")
    }
}

fn safe_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    let s = if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    };
    s.chars().take(150).collect()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("\nerror: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = Args::parse();

    // Speed profiles, measured rather than guessed.
    //
    // `ultrafast` is a trap here: it encodes faster but emits a much larger
    // bitstream, and writing that costs more than the encoder saves -- it
    // measured slower end to end than `superfast`.  So the fast profile cuts
    // pixels and frames instead, which reduces work everywhere.
    if args.fast {
        args.crf = 30;
        if args.width == 1280 {
            args.width = 960;
        }
        if args.fps == 12.0 {
            args.fps = 8.0;
        }
    } else if args.best {
        args.preset = "veryfast".into();
        args.crf = 23;
    }

    let token = args
        .token
        .clone()
        .or_else(|| std::env::var("UNACADEMY_TOKEN").ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let Some(token) = token else {
        bail!(
            "no token: pass it as the second argument or set UNACADEMY_TOKEN.\n\
             It is the 'accessToken' cookie on unacademy.com."
        );
    };

    let rep = Reporter::new(args.quiet);

    // Check the toolchain before touching the network: discovering a missing
    // ffmpeg after downloading hundreds of megabytes would be miserable.
    export::require_ffmpeg()?;

    std::fs::create_dir_all(&args.out)?;

    let uid = api::parse_class_uid(&args.url)?;
    rep.say(&format!("Resolving {uid} ..."));

    let client = api::Client::new(&token);
    let info = client.class_info(&args.url)?;

    let stem = safe_name(
        format!("{} - {}", info.title, info.educator).trim_end_matches(" - "),
        &info.class_uid,
    );
    let base = args.out.join(&stem);
    let final_mp4 = base.with_extension("mp4");

    rep.say(&format!("  {}", info.title));
    rep.say(&format!(
        "  {}  |  {:.0} min  |  {}",
        info.educator,
        info.duration / 60.0,
        info.media_uid
    ));

    // -- events --------------------------------------------------------------
    let data_path = base.with_extension("events.json");
    let text = if data_path.is_file() {
        std::fs::read_to_string(&data_path)?
    } else {
        rep.say("Fetching board events ...");
        let t = client.get_text(&info.data_url())?;
        std::fs::write(&data_path, &t)?;
        t
    };
    let events = model::load_events(&text)?;
    rep.say(&format!("  {} events", events.len()));

    // -- media ---------------------------------------------------------------
    let webm = base.with_extension("webm");
    let media_url = info.media_url();

    // A complete file from an earlier run is worth keeping: re-downloading
    // hundreds of megabytes to render the same class again is pure waste.
    let have = std::fs::metadata(&webm).map(|m| m.len()).unwrap_or(0);
    let remote = download::probe(&client.agent, &media_url)
        .map(|(size, _)| size)
        .unwrap_or(0);

    if have > 0 && have == remote {
        rep.say(&format!("Media already downloaded ({:.0} MB)", have as f64 / 1e6));
    } else {
        rep.say(&format!(
            "Downloading media on {} connections ...",
            args.connections
        ));
        let bytes = download::download(
            client.agent.clone(),
            &media_url,
            &webm,
            args.connections,
            |p| rep.line(&format!("  {}", download::bar(p))),
        )?;
        rep.done(&format!("  downloaded {:.0} MB", bytes as f64 / 1e6));
    }

    // -- render --------------------------------------------------------------
    let jobs = if args.jobs > 0 {
        args.jobs
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    };
    let height = ((args.width as f64 * 9.0 / 16.0).round() as u32) & !1;

    let opts = export::Options {
        fps: args.fps,
        width: args.width,
        height,
        crf: args.crf,
        preset: args.preset.clone(),
        pip: !args.no_pip,
        zoom: !args.no_zoom,
        start: args.start,
        end: args.end,
        ..Default::default()
    };

    rep.say(&format!(
        "Rendering board at {}x{} @ {}fps on {jobs} threads ...",
        args.width,
        height,
        trim_num(args.fps)
    ));

    let slides = SlideCache::new(args.width);
    let started = Instant::now();
    export::export(
        &events,
        &final_mp4,
        Some(&webm),
        &opts,
        info.duration,
        jobs,
        &slides,
        |done, total| {
            let secs = started.elapsed().as_secs_f64().max(1e-6);
            let rate = done as f64 / secs;
            let eta = if rate > 0.0 {
                ((total - done) as f64 / rate) as u64
            } else {
                0
            };
            rep.line(&format!(
                "  frame {done}/{total} ({:5.1}%)  {rate:6.1} fps  ETA {}m{:02}s",
                100.0 * done as f64 / total as f64,
                eta / 60,
                eta % 60
            ));
        },
    )?;
    rep.done(&format!(
        "  rendered in {:.0}s",
        started.elapsed().as_secs_f64()
    ));

    if !args.keep {
        let _ = std::fs::remove_file(&webm);
        let _ = std::fs::remove_file(&data_path);
    }

    let size = std::fs::metadata(&final_mp4).map(|m| m.len()).unwrap_or(0);
    rep.say(&format!(
        "\n-> {}  ({:.0} MB)",
        final_mp4.display(),
        size as f64 / 1e6
    ));
    Ok(())
}
