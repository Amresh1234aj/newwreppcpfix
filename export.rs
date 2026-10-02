//! Render the class to a video file.
//!
//! Only the board is drawn here. Frames go to ffmpeg as raw RGB over a pipe,
//! and ffmpeg decodes the webm, scales the webcam, overlays it and takes the
//! audio — all in C, on its own threads, while we render the next frames.
//!
//! Encoding is roughly 90% of the work and x264 already uses every core, so
//! splitting the timeline across processes buys much less than it looks like
//! it should. It is still worth doing: the segments are independent given the
//! state at their start, and ffmpeg concatenates them without re-encoding.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use crate::assets::SlideCache;
use crate::model::{Event, P_DOC};
use crate::render::{BoardRenderer, Theme};
use crate::state::{build_at, ReplayState};

#[derive(Clone, Debug)]
pub struct Options {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub crf: u32,
    pub preset: String,
    pub pip: bool,
    pub pip_scale: f64,
    pub pip_corner: String,
    pub zoom: bool,
    pub audio: bool,
    pub threads: usize,
    pub start: f64,
    pub end: Option<f64>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fps: 12.0,
            width: 1280,
            height: 720,
            crf: 28,
            preset: "superfast".into(),
            pip: true,
            pip_scale: 0.22,
            pip_corner: "br".into(),
            zoom: true,
            audio: true,
            threads: 0,
            start: 0.0,
            end: None,
        }
    }
}

impl Options {
    fn theme(&self) -> Theme {
        let mut t = Theme::new(self.width, self.height);
        t.apply_zoom = self.zoom;
        t
    }
}

/// Locate ffmpeg: beside the binary first, so a portable copy is
/// self-contained, then `$UADL_FFMPEG`, then `PATH`.
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("UADL_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [dir.join("ffmpeg"), dir.join("bin/ffmpeg")] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    // Walk PATH ourselves rather than relying on the shell, so the check and
    // the later spawn agree on what will actually run.
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("ffmpeg");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn ffmpeg_path() -> PathBuf {
    find_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// Confirm ffmpeg is usable before doing any work, and say how to get it if
/// not. Failing here beats failing after a 400 MB download.
pub fn require_ffmpeg() -> Result<PathBuf> {
    let Some(path) = find_ffmpeg() else {
        bail!("{}", install_hint());
    };
    match Command::new(&path).arg("-version").output() {
        Ok(out) if out.status.success() => Ok(path),
        _ => bail!(
            "found ffmpeg at {} but could not run it.\n{}",
            path.display(),
            install_hint()
        ),
    }
}

fn install_hint() -> String {
    let manager = [
        ("/usr/bin/apt-get", "sudo apt install ffmpeg"),
        ("/usr/bin/dnf", "sudo dnf install ffmpeg"),
        ("/usr/bin/pacman", "sudo pacman -S ffmpeg"),
        ("/usr/bin/zypper", "sudo zypper install ffmpeg"),
        ("/usr/bin/apk", "sudo apk add ffmpeg"),
        ("/opt/homebrew/bin/brew", "brew install ffmpeg"),
        ("/usr/local/bin/brew", "brew install ffmpeg"),
    ]
    .iter()
    .find(|(probe, _)| Path::new(probe).exists())
    .map(|(_, cmd)| *cmd)
    .unwrap_or("install ffmpeg with your package manager");

    format!(
        "ffmpeg is required but was not found.\n\n  {manager}\n\n\
         Alternatively drop an `ffmpeg` binary next to uadl, or point\n\
         UADL_FFMPEG at one. Static builds: https://johnvansickle.com/ffmpeg/"
    )
}

/// URLs of every slide visible between `start` and `end`.
fn slides_shown(events: &[Event], start: f64, end: f64) -> Vec<String> {
    let (lo, hi) = ((start * 1e6) as i64, (end * 1e6) as i64);
    let final_state = build_at(events, hi);

    let mut urls: Vec<String> = Vec::new();
    let mut push = |u: Option<&String>| {
        if let Some(u) = u {
            if !urls.contains(u) {
                urls.push(u.clone());
            }
        }
    };

    push(build_at(events, lo)
        .active_slide()
        .and_then(|s| s.url.as_ref()));

    for ev in events {
        if ev.plugin == P_DOC && ev.kind == "sc" && ev.p_time >= lo && ev.p_time <= hi {
            if let Some(idx) = ev.i("s") {
                push(final_state.slides.get(&idx).and_then(|s| s.url.as_ref()));
            }
        }
    }
    urls
}

fn overlay_filter(o: &Options) -> String {
    // `-2` for the height keeps the source aspect while staying even, which
    // yuv420p requires.
    let width = ((o.width as f64 * o.pip_scale) as u32 / 2 * 2).max(2);
    let border = ((o.width as f64 / 640.0).round() as u32).max(1);
    let margin = ((o.width as f64 / 71.0).round() as u32).max(2);

    let x = if matches!(o.pip_corner.as_str(), "tr" | "br") {
        format!("W-w-{margin}")
    } else {
        margin.to_string()
    };
    let y = if matches!(o.pip_corner.as_str(), "bl" | "br") {
        format!("H-h-{margin}")
    } else {
        margin.to_string()
    };

    // `setpts=PTS-STARTPTS` matters. With `-ss`, the webcam stream can
    // start a fraction of a frame after the board does, and `overlay`
    // passes the base through untouched until its second input produces
    // something. Rendering in parallel segments turned that into a missing
    // inset on the first frame of every segment. Rebasing the timestamps to
    // zero lines the two up. `eof_action=pass` keeps the board flowing if
    // the webcam track ends early.
    format!(
        "[1:v]setpts=PTS-STARTPTS,scale={width}:-2,\
         pad=iw+{}:ih+{}:{border}:{border}:white[pip];\
         [0:v][pip]overlay={x}:{y}:shortest=0:eof_action=pass:repeatlast=1[v]",
        border * 2,
        border * 2
    )
}

fn ffmpeg_command(out: &Path, o: &Options, media: Option<&Path>, start: f64, end: f64) -> Command {
    let mut cmd = Command::new(ffmpeg_path());
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    // Frames arrive already converted. Handing ffmpeg rgb24 instead makes it
    // run swscale per frame over twice the bytes, which measured slower than
    // the encoding itself.
    cmd.args(["-f", "rawvideo", "-pix_fmt", "yuv420p"]);
    cmd.args(["-s", &format!("{}x{}", o.width, o.height)]);
    cmd.args(["-r", &o.fps.to_string(), "-i", "-"]);

    let use_media = media.is_some() && (o.audio || o.pip);
    let use_pip = media.is_some() && o.pip;

    if let (true, Some(m)) = (use_media, media) {
        // Seek before -i so both streams line up with the first frame.
        if start > 0.0 {
            cmd.args(["-ss", &format!("{start:.3}")]);
        }
        cmd.arg("-i").arg(m);
    }

    if use_pip {
        cmd.args(["-filter_complex", &overlay_filter(o), "-map", "[v]"]);
    } else {
        cmd.args(["-map", "0:v:0"]);
    }
    if media.is_some() && o.audio {
        cmd.args(["-map", "1:a:0?"]);
    }

    cmd.args(["-c:v", "libx264", "-preset", &o.preset]);
    cmd.args(["-crf", &o.crf.to_string()]);
    cmd.args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);
    if o.threads > 0 {
        cmd.args(["-threads", &o.threads.to_string()]);
    }
    if media.is_some() && o.audio {
        cmd.args(["-c:a", "aac", "-b:a", "128k", "-shortest"]);
    }
    cmd.args(["-t", &format!("{:.3}", end - start)]);
    cmd.arg(out);
    cmd.stdin(Stdio::piped());
    cmd
}

/// Render `[first_frame, last_frame)` into `out`.
fn render_segment(
    events: &[Event],
    out: &Path,
    media: Option<&Path>,
    o: &Options,
    first_frame: usize,
    last_frame: usize,
    slides: &SlideCache,
    done: &AtomicUsize,
) -> Result<()> {
    let start = first_frame as f64 / o.fps;
    let end = last_frame as f64 / o.fps;
    let frames = last_frame - first_frame;

    let mut state = if first_frame == 0 {
        ReplayState::new()
    } else {
        build_at(events, (start * 1e6) as i64)
    };

    let mut renderer = BoardRenderer::new(o.theme(), slides.clone());

    let mut child = ffmpeg_command(out, o, media, start, end)
        .spawn()
        .context("spawning ffmpeg")?;
    let mut stdin = child.stdin.take().expect("piped stdin");

    for i in 0..frames {
        let t = start + i as f64 / o.fps;
        state.advance(events, (t * 1_000_000.0) as i64);
        let frame = renderer.render(&state);
        // Planar: all of Y, then U, then V.
        if stdin.write_all(&frame.y).is_err()
            || stdin.write_all(&frame.u).is_err()
            || stdin.write_all(&frame.v).is_err()
        {
            break; // ffmpeg went away; its exit status is the real error
        }
        done.fetch_add(1, Ordering::Relaxed);
    }

    drop(stdin);
    let status = child.wait()?;
    if !status.success() {
        bail!("ffmpeg exited with {status}");
    }
    Ok(())
}

/// Render across threads, then concatenate without re-encoding.
pub fn export(
    events: &[Event],
    out_path: &Path,
    media: Option<&Path>,
    o: &Options,
    duration: f64,
    jobs: usize,
    slides: &SlideCache,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<()> {
    let total_s = if duration > 0.0 {
        duration
    } else {
        events.last().map(|e| e.t()).unwrap_or(0.0)
    };
    let end = o.end.map(|e| e.min(total_s)).unwrap_or(total_s);
    let start = o.start.max(0.0);
    if end <= start {
        bail!("empty time range: {start} .. {end}");
    }

    let first = (start * o.fps) as usize;
    let last = (end * o.fps) as usize;
    let total_frames = last - first;

    // One slide fetch shared by every worker, before they all race for it.
    slides.prefetch(&slides_shown(events, start, end));

    let jobs = jobs.max(1);
    // Always stage on local disk: ffmpeg writes the muxed output
    // incrementally, and on a translated mount (WSL /mnt/c, SMB) each of
    // those small writes pays the crossing — measured 1.8x slower.
    let tmpdir = std::env::temp_dir().join(format!("uadl-{}", std::process::id()));
    std::fs::create_dir_all(&tmpdir)?;

    let result = (|| -> Result<()> {
        let done = Arc::new(AtomicUsize::new(0));
        let mut opts = o.clone();
        if opts.threads == 0 && jobs > 1 {
            // Each worker runs its own ffmpeg, and libx264 grabs every core by
            // default. Give each a fair share instead of N-way contention.
            opts.threads = (std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                / jobs)
                .max(1);
        }

        let edges: Vec<usize> = (0..=jobs)
            .map(|i| first + (i * total_frames) / jobs)
            .collect();

        let parts: Vec<PathBuf> = (0..jobs)
            .map(|i| tmpdir.join(format!("seg{i:03}.mp4")))
            .collect();

        std::thread::scope(|scope| -> Result<()> {
            let mut handles = Vec::new();
            for i in 0..jobs {
                if edges[i + 1] <= edges[i] {
                    continue;
                }
                let (a, b) = (edges[i], edges[i + 1]);
                let out = parts[i].clone();
                let opts = opts.clone();
                let done = done.clone();
                let slides = slides.clone();
                handles.push(scope.spawn(move || {
                    render_segment(events, &out, media, &opts, a, b, &slides, &done)
                }));
            }

            while !handles.iter().all(|h| h.is_finished()) {
                on_progress(done.load(Ordering::Relaxed).min(total_frames), total_frames);
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            on_progress(total_frames, total_frames);

            for h in handles {
                h.join().map_err(|_| anyhow::anyhow!("render thread panicked"))??;
            }
            Ok(())
        })?;

        let present: Vec<&PathBuf> = parts.iter().filter(|p| p.is_file()).collect();
        if present.is_empty() {
            bail!("no segments were rendered");
        }

        let staged = tmpdir.join("final.mp4");
        if present.len() == 1 {
            std::fs::rename(present[0], &staged)?;
        } else {
            concat(&present, &staged)?;
        }
        place(&staged, out_path)?;
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// Join segments with the concat demuxer — stream copy, no re-encode.
fn concat(parts: &[&PathBuf], out: &Path) -> Result<()> {
    let listing = out.with_extension("concat.txt");
    let body: String = parts
        .iter()
        .map(|p| format!("file '{}'\n", p.display().to_string().replace('\'', r"'\''")))
        .collect();
    std::fs::write(&listing, body)?;

    let status = Command::new(ffmpeg_path())
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "concat", "-safe", "0"])
        .arg("-i")
        .arg(&listing)
        .args(["-c", "copy", "-fflags", "+genpts", "-movflags", "+faststart"])
        .arg(out)
        .status()?;
    let _ = std::fs::remove_file(&listing);

    if !status.success() {
        bail!("concat failed with {status}");
    }
    Ok(())
}

/// Move the finished render into place: a rename when possible, a copy across
/// filesystems.
fn place(staged: &Path, out: &Path) -> Result<()> {
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    if std::fs::rename(staged, out).is_ok() {
        return Ok(());
    }
    std::fs::copy(staged, out)?;
    let _ = std::fs::remove_file(staged);
    Ok(())
}
