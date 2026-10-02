//! Slide image fetching and caching.
//!
//! Slides are PDF pages behind an imgix-style CDN: the same URL with `?page=N`
//! plus transform parameters renders that page at any width. We ask for the
//! board width so strokes land on a crisp background.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use image::imageops::FilterType;
use tiny_skia::Pixmap;

pub const PLAYER_ORIGIN: &str = "https://player.uacdn.net";
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct SlideCache {
    dir: PathBuf,
    agent: Arc<ureq::Agent>,
    width: u32,
}

impl SlideCache {
    pub fn new(width: u32) -> Self {
        let dir = cache_dir();
        let _ = fs::create_dir_all(&dir);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .user_agent(UA)
            .build();
        SlideCache {
            dir,
            agent: Arc::new(agent),
            width,
        }
    }

    fn path_for(&self, url: &str) -> PathBuf {
        let digest = sha1_smol::Sha1::from(format!("{url}|{}", self.width))
            .digest()
            .to_string();
        self.dir.join(format!("{digest}.img"))
    }

    /// Fetch (or read from cache) and scale to `w x h`.
    pub fn get(&self, url: &str, w: u32, h: u32) -> Option<Pixmap> {
        let bytes = self.bytes(url)?;
        let img = image::load_from_memory(&bytes).ok()?;
        let img = img.resize_exact(w, h, FilterType::Triangle).to_rgba8();

        let mut pix = Pixmap::new(w, h)?;
        for (dst, src) in pix.pixels_mut().iter_mut().zip(img.pixels()) {
            let [r, g, b, a] = src.0;
            *dst = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
        }
        Some(pix)
    }

    /// Download once, then serve from disk on every later run.
    pub fn bytes(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.path_for(url);
        if let Ok(data) = fs::read(&path) {
            if !data.is_empty() {
                return Some(data);
            }
        }

        let data = self.download(url)?;
        // Write via a temp file so parallel workers never see a partial image.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if fs::write(&tmp, &data).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
        Some(data)
    }

    fn download(&self, url: &str) -> Option<Vec<u8>> {
        for candidate in [transform_url(url, self.width), url.to_string()] {
            let res = self
                .agent
                .get(&candidate)
                .set("Referer", &format!("{PLAYER_ORIGIN}/"))
                .set("Origin", PLAYER_ORIGIN)
                .set("Accept", "image/avif,image/webp,image/*,*/*;q=0.8")
                .call();
            if let Ok(resp) = res {
                let mut buf = Vec::new();
                if resp.into_reader().read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
        None
    }

    /// Warm the cache in parallel — this is the longest stall before a render
    /// starts, and it is pure I/O.
    pub fn prefetch(&self, urls: &[String]) {
        use rayon::prelude::*;
        urls.par_iter().for_each(|u| {
            let _ = self.bytes(u);
        });
    }
}

use std::io::Read;

/// Ask the image CDN for this slide at `width` pixels.
fn transform_url(url: &str, width: u32) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, q),
        None => (url, ""),
    };
    let mut parts: Vec<String> = query
        .split('&')
        .filter(|kv| {
            !kv.is_empty()
                && !matches!(
                    kv.split('=').next().unwrap_or(""),
                    "w" | "fm" | "fit" | "auto"
                )
        })
        .map(str::to_string)
        .collect();
    parts.push(format!("w={width}"));
    parts.push("fm=webp".into());
    parts.push("fit=clip".into());
    parts.push("auto=compress".into());
    format!("{base}?{}", parts.join("&"))
}

fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        return Path::new(&dir).join("uadl/slides");
    }
    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home).join(".cache/uadl/slides");
    }
    std::env::temp_dir().join("uadl-slides")
}
