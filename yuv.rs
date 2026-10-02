//! RGBA -> YUV420p, done here rather than in ffmpeg.
//!
//! Handing ffmpeg `rgb24` turned out to be the single biggest cost in the
//! pipeline — bigger than encoding. Measured at 1280x720 with 1200 frames:
//!
//! ```text
//! rgb24   -> null (no encode)   123.8 fps      <- the ceiling
//! rgb24   -> x264 superfast     116.1 fps
//! yuv420p -> x264 superfast     288.6 fps
//! ```
//!
//! ffmpeg was running a full-frame colour conversion per frame on twice the
//! pipe bytes. Converting here instead means we can also *cache* the result:
//! the board only changes a few thousand times in a class, so most frames are
//! a memcpy plus a repaint of the small square the cursor sits in.
//!
//! Coefficients are BT.601 limited range, matching what swscale produced for
//! `rgb24 -> yuv420p`, so colours are unchanged from the previous pipeline.

use tiny_skia::PixmapRef;

/// A planar YUV420p frame, laid out as ffmpeg wants it on the wire.
#[derive(Clone)]
pub struct YuvFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl YuvFrame {
    pub fn new(width: u32, height: u32) -> Self {
        // yuv420p needs even dimensions; callers already round, but be safe.
        let (w, h) = (width as usize, height as usize);
        YuvFrame {
            width,
            height,
            y: vec![16; w * h],
            u: vec![128; (w / 2) * (h / 2)],
            v: vec![128; (w / 2) * (h / 2)],
        }
    }

    pub fn copy_from(&mut self, other: &YuvFrame) {
        self.y.copy_from_slice(&other.y);
        self.u.copy_from_slice(&other.u);
        self.v.copy_from_slice(&other.v);
    }
}

/// Un-premultiply one pixel to straight RGB.
#[inline(always)]
fn straight(r: u8, g: u8, b: u8, a: u8) -> (i32, i32, i32) {
    match a {
        255 => (r as i32, g as i32, b as i32),
        0 => (0, 0, 0),
        _ => {
            let inv = 255.0 / a as f32;
            (
                (r as f32 * inv).min(255.0) as i32,
                (g as f32 * inv).min(255.0) as i32,
                (b as f32 * inv).min(255.0) as i32,
            )
        }
    }
}

// BT.601 limited range, in 16.16 fixed point.
#[inline(always)]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (((16829 * r + 33039 * g + 6416 * b + 32768) >> 16) + 16).clamp(16, 235) as u8
}

#[inline(always)]
fn chroma_u(r: i32, g: i32, b: i32) -> u8 {
    (((-9714 * r - 19070 * g + 28784 * b + 32768) >> 16) + 128).clamp(16, 240) as u8
}

#[inline(always)]
fn chroma_v(r: i32, g: i32, b: i32) -> u8 {
    (((28784 * r - 24103 * g - 4681 * b + 32768) >> 16) + 128).clamp(16, 240) as u8
}

/// Convert a rectangle of `src` into `dst`, leaving the rest untouched.
///
/// `x0`/`y0` are snapped down and `x1`/`y1` up to even coordinates so the
/// 2x2 chroma blocks stay aligned.
pub fn convert_region(src: &PixmapRef, dst: &mut YuvFrame, rect: (u32, u32, u32, u32)) {
    let (w, h) = (dst.width as usize, dst.height as usize);
    let (mut x0, mut y0, mut x1, mut y1) = rect;
    x0 &= !1;
    y0 &= !1;
    x1 = (x1 + 1) & !1;
    y1 = (y1 + 1) & !1;
    let (x0, y0) = (x0 as usize, y0 as usize);
    let (x1, y1) = ((x1 as usize).min(w), (y1 as usize).min(h));
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let px = src.pixels();
    let cw = w / 2;

    // Walk 2x2 blocks: one chroma sample per block, four luma samples.
    let mut by = y0;
    while by < y1 {
        let mut bx = x0;
        while bx < x1 {
            let mut acc = [0i32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let (x, y) = (bx + dx, by + dy);
                    let p = px[y * w + x];
                    let (r, g, b) = straight(p.red(), p.green(), p.blue(), p.alpha());
                    dst.y[y * w + x] = luma(r, g, b);
                    acc[0] += r;
                    acc[1] += g;
                    acc[2] += b;
                }
            }
            let (r, g, b) = (acc[0] / 4, acc[1] / 4, acc[2] / 4);
            let ci = (by / 2) * cw + bx / 2;
            dst.u[ci] = chroma_u(r, g, b);
            dst.v[ci] = chroma_v(r, g, b);
            bx += 2;
        }
        by += 2;
    }
}

/// Convert the whole frame.
pub fn convert(src: &PixmapRef, dst: &mut YuvFrame) {
    convert_region(src, dst, (0, 0, dst.width, dst.height));
}
