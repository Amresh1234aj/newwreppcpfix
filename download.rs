//! Multi-connection HTTP download.
//!
//! The class CDN serves byte ranges and throttles per connection, not per
//! client: one stream tops out near 0.3 MB/s while sixteen reach 16 MB/s on
//! the same file. So the file is split into ranges, each fetched on its own
//! thread and written straight into its slot in a preallocated file.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Result};

use crate::assets::PLAYER_ORIGIN;

const BLOCK: usize = 1 << 18; // 256 KiB socket reads
const MIN_CHUNK: u64 = 1 << 20; // never split below 1 MiB per connection

pub struct Progress {
    pub done: Arc<AtomicU64>,
    pub total: u64,
    pub started: Instant,
}

impl Progress {
    pub fn rate(&self) -> f64 {
        let secs = self.started.elapsed().as_secs_f64();
        if secs > 0.0 {
            self.done.load(Ordering::Relaxed) as f64 / secs
        } else {
            0.0
        }
    }

    pub fn eta(&self) -> f64 {
        let r = self.rate();
        if r > 0.0 {
            (self.total - self.done.load(Ordering::Relaxed).min(self.total)) as f64 / r
        } else {
            0.0
        }
    }
}

/// `(size, supports_ranges)` for `url`.
pub fn probe(agent: &ureq::Agent, url: &str) -> Result<(u64, bool)> {
    let resp = agent
        .get(url)
        .set("Range", "bytes=0-0")
        .set("Origin", PLAYER_ORIGIN)
        .set("Referer", &format!("{PLAYER_ORIGIN}/"))
        .call()?;

    if resp.status() == 206 {
        if let Some(cr) = resp.header("Content-Range") {
            if let Some(total) = cr.rsplit('/').next().and_then(|s| s.parse::<u64>().ok()) {
                return Ok((total, true));
            }
        }
    }
    let len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    Ok((len, false))
}

/// Fetch `url` to `dest` over `connections` parallel range requests.
pub fn download(
    agent: Arc<ureq::Agent>,
    url: &str,
    dest: &std::path::Path,
    connections: usize,
    mut on_progress: impl FnMut(&Progress),
) -> Result<u64> {
    let (total, ranged) = probe(&agent, url)?;
    if total == 0 {
        bail!("server did not report a size for {url}");
    }

    let done = Arc::new(AtomicU64::new(0));
    let progress = Progress {
        done: done.clone(),
        total,
        started: Instant::now(),
    };

    {
        // Preallocate so every worker can seek straight to its slot.
        let f = File::create(dest)?;
        f.set_len(total)?;
    }

    let workers = if ranged {
        connections.max(1).min(((total + MIN_CHUNK - 1) / MIN_CHUNK) as usize)
    } else {
        1
    };
    let span = total / workers as u64;

    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for i in 0..workers {
            let start = i as u64 * span;
            let end = if i == workers - 1 {
                total - 1
            } else {
                (i as u64 + 1) * span - 1
            };
            let agent = agent.clone();
            let done = done.clone();
            let url = url.to_string();
            let dest = dest.to_path_buf();

            handles.push(scope.spawn(move || -> Result<()> {
                let mut file = OpenOptions::new().write(true).open(&dest)?;
                file.seek(SeekFrom::Start(start))?;

                let mut req = agent
                    .get(&url)
                    .set("Origin", PLAYER_ORIGIN)
                    .set("Referer", &format!("{PLAYER_ORIGIN}/"));
                if ranged {
                    req = req.set("Range", &format!("bytes={start}-{end}"));
                }

                let mut reader = req.call()?.into_reader();
                let mut buf = vec![0u8; BLOCK];
                loop {
                    let n = reader.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buf[..n])?;
                    done.fetch_add(n as u64, Ordering::Relaxed);
                }
                Ok(())
            }));
        }

        // Report while the workers run.
        while !handles.iter().all(|h| h.is_finished()) {
            on_progress(&progress);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        on_progress(&progress);

        for h in handles {
            h.join().map_err(|_| anyhow::anyhow!("download thread panicked"))??;
        }
        Ok(())
    })?;

    Ok(total)
}

pub fn human(n: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n;
    for (i, unit) in UNITS.iter().enumerate() {
        if v.abs() < 1024.0 || i == UNITS.len() - 1 {
            return format!("{v:.1}{unit}");
        }
        v /= 1024.0;
    }
    format!("{v:.1}GB")
}

pub fn bar(p: &Progress) -> String {
    let done = p.done.load(Ordering::Relaxed).min(p.total);
    let frac = if p.total > 0 {
        done as f64 / p.total as f64
    } else {
        0.0
    };
    let width = 28usize;
    let filled = (frac * width as f64) as usize;
    let eta = p.eta() as u64;
    format!(
        "[{}{}] {:5.1}%  {}/{}  {}/s  ETA {}m{:02}s",
        "=".repeat(filled),
        " ".repeat(width - filled),
        frac * 100.0,
        human(done as f64),
        human(p.total as f64),
        human(p.rate()),
        eta / 60,
        eta % 60
    )
}
