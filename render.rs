//! Draw the board.
//!
//! Layering matches the browser player:
//!
//! ```text
//! slide image -> highlighter -> ink -> pointer
//! ```
//!
//! Highlighter sits *under* ink so writing over a highlight stays readable,
//! and eraser strokes clear both annotation layers.
//!
//! Between two frames the board usually has not changed at all — only the
//! cursor moved — so a fully composited board is cached and most frames cost
//! one memcpy plus the laser dot.

use std::collections::HashMap;

use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint,
    PixmapRef, Rect, Stroke as SkStroke, Transform,
};

use crate::assets::SlideCache;
use crate::state::{Canvas, ReplayState};
use crate::yuv::{self, YuvFrame};

/// Stroke widths in pixels at a 1280-wide board, scaled linearly for other
/// sizes and multiplied by the thickness the educator selected.
const MARKER_PX: f32 = 4.0;
const HIGHLIGHT_PX: f32 = 26.0;
const ERASER_PX: f32 = 46.0;
const HIGHLIGHT_ALPHA: f32 = 0.38;
const POINTER_PX: f32 = 9.0;
const POINTER_COLOR: [u8; 3] = [255, 64, 64];

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub width: u32,
    pub height: u32,
    pub show_pointer: bool,
    /// Follow the educator's zoom/pan, as the browser player does.
    pub apply_zoom: bool,
}

impl Theme {
    pub fn new(width: u32, height: u32) -> Self {
        Theme {
            width,
            height,
            show_pointer: true,
            apply_zoom: true,
        }
    }

    fn scale(&self) -> f32 {
        self.width as f32 / 1280.0
    }
}

/// The player's own transform for a canvas: `(scale, tx, ty)`.
///
/// Lifted from the player bundle, where the slide, ink and pointer layers all
/// receive the same CSS transform with the default 50%/50% origin:
///
/// ```text
/// transform: translate(translationX * W, translationY * H) scale(canvasScale)
/// setTranslation()     -> getTranslationLimits(pan * scale, scale)
/// getTranslationLimits -> clamp(v, -(s - 1) / 2, (s - 1) / 2)
/// ```
///
/// The clamp keeps the viewport inside the page: at either limit the visible
/// window lands exactly on an edge, so a zoomed board never shows margin.
pub fn transform_of(canvas: &Canvas) -> (f32, f32, f32) {
    let s = canvas.zoom;
    if s <= 1.0 + 1e-6 {
        return (1.0, 0.0, 0.0);
    }
    let limit = 0.5 * (s - 1.0);
    let clamp = |v: f32| v.clamp(-limit, limit);
    (s, clamp(canvas.pan.0 * s), clamp(canvas.pan.1 * s))
}

/// Page coords -> screen coords, both normalised.
fn project(canvas: &Canvas, x: f32, y: f32) -> (f32, f32) {
    let (s, tx, ty) = transform_of(canvas);
    (0.5 + s * (x - 0.5) + tx, 0.5 + s * (y - 0.5) + ty)
}

/// Identifies a cached board: if this is unchanged, so is the board.
#[derive(PartialEq, Clone)]
struct FrameKey {
    slide: i64,
    revision: u64,
    transform: (u32, u32, u32),
    url: Option<String>,
}

pub struct BoardRenderer {
    pub theme: Theme,
    slides: SlideCache,
    /// Scaled slide backgrounds, keyed by URL.
    backgrounds: HashMap<String, Pixmap>,
    cached: Option<(FrameKey, Pixmap)>,
    scratch: Pixmap,
    /// The cached board, already converted. Most frames only need the small
    /// square under the cursor repainted on top of a copy of this.
    board_yuv: YuvFrame,
    out_yuv: YuvFrame,
    /// Rectangle the previous frame's cursor dirtied, so it can be restored.
    dirty: Option<(u32, u32, u32, u32)>,
}

impl BoardRenderer {
    pub fn new(theme: Theme, slides: SlideCache) -> Self {
        BoardRenderer {
            scratch: Pixmap::new(theme.width, theme.height).expect("pixmap"),
            board_yuv: YuvFrame::new(theme.width, theme.height),
            out_yuv: YuvFrame::new(theme.width, theme.height),
            dirty: None,
            theme,
            slides,
            backgrounds: HashMap::new(),
            cached: None,
        }
    }

    fn background(&mut self, state: &ReplayState) -> Pixmap {
        let (w, h) = (self.theme.width, self.theme.height);
        let slide = state.active_slide();

        if let Some(url) = slide.and_then(|s| s.url.as_deref()) {
            if !self.backgrounds.contains_key(url) {
                if let Some(img) = self.slides.get(url, w, h) {
                    self.backgrounds.insert(url.to_string(), img);
                }
            }
            if let Some(img) = self.backgrounds.get(url) {
                return img.clone();
            }
        }

        let bg = slide.map(|s| s.background).unwrap_or([255, 255, 255]);
        let mut p = Pixmap::new(w, h).expect("pixmap");
        p.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
        p
    }

    /// Draw one canvas's strokes onto `target`, in player order.
    fn draw_strokes(&self, target: &mut Pixmap, canvas: &Canvas) {
        let (w, h) = (self.theme.width as f32, self.theme.height as f32);
        let scale = self.theme.scale();

        // Highlighter first so ink stays readable on top of it. Erasers are
        // replayed in sequence against whatever is already down, which is
        // what makes partial erasing behave.
        for pass in 0..2 {
            for stroke in canvas.visible_strokes() {
                let want_highlight = pass == 0;
                if stroke.is_erase() {
                    // An eraser clears both layers, so it runs in both passes.
                } else if stroke.is_highlight() != want_highlight {
                    continue;
                }

                let Some(path) = build_path(&stroke.points, w, h) else {
                    continue;
                };

                let (width, color, blend) = if stroke.is_erase() {
                    (
                        ERASER_PX * scale * stroke.width,
                        Color::TRANSPARENT,
                        BlendMode::Clear,
                    )
                } else if stroke.is_highlight() {
                    (
                        HIGHLIGHT_PX * scale * stroke.width,
                        Color::from_rgba8(
                            stroke.color[0],
                            stroke.color[1],
                            stroke.color[2],
                            (255.0 * HIGHLIGHT_ALPHA) as u8,
                        ),
                        BlendMode::SourceOver,
                    )
                } else {
                    (
                        MARKER_PX * scale * stroke.width,
                        Color::from_rgba8(stroke.color[0], stroke.color[1], stroke.color[2], 255),
                        BlendMode::SourceOver,
                    )
                };

                let mut paint = Paint::default();
                paint.set_color(color);
                paint.anti_alias = true;
                paint.blend_mode = blend;

                let sk = SkStroke {
                    width: width.max(1.0),
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Default::default()
                };
                target.stroke_path(&path, &paint, &sk, Transform::identity(), None);
            }
        }
    }

    /// Apply the educator's zoom/pan to a finished board.
    fn apply_zoom(&self, board: &Pixmap, canvas: &Canvas) -> Option<Pixmap> {
        let (s, tx, ty) = transform_of(canvas);
        if !self.theme.apply_zoom || (s <= 1.0 + 1e-6 && tx == 0.0 && ty == 0.0) {
            return None;
        }
        let (w, h) = (self.theme.width as f32, self.theme.height as f32);
        let mut out = Pixmap::new(self.theme.width, self.theme.height)?;
        let t = Transform::from_row(
            s,
            0.0,
            0.0,
            s,
            (0.5 - 0.5 * s + tx) * w,
            (0.5 - 0.5 * s + ty) * h,
        );
        let mut paint = PixmapPaint::default();
        paint.quality = tiny_skia::FilterQuality::Bilinear;
        out.draw_pixmap(0, 0, board.as_ref(), &paint, t, None);
        Some(out)
    }

}

/// Draw the laser dot, returning the rectangle it touched so the caller can
/// reconvert just that patch instead of the whole frame.
fn draw_pointer_into(
    frame: &mut Pixmap,
    canvas: &Canvas,
    t_us: i64,
    theme: &Theme,
) -> Option<(u32, u32, u32, u32)> {
    {
        if !theme.show_pointer {
            return None;
        }
        let alpha = canvas.pointer_alpha(t_us);
        if alpha <= 0.02 {
            return None;
        }
        let Some((mut x, mut y)) = canvas.pointer else {
            return None;
        };
        if theme.apply_zoom {
            let (px, py) = project(canvas, x, y);
            x = px;
            y = py;
        }
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return None;
        }

        let cx = x * theme.width as f32;
        let cy = y * theme.height as f32;
        let r = (POINTER_PX * theme.scale()).max(3.0);

        for (radius, a) in [(r * 1.9, 0.20 * alpha), (r, 0.75 * alpha)] {
            let mut pb = PathBuilder::new();
            pb.push_circle(cx, cy, radius);
            let Some(path) = pb.finish() else { continue };
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(
                POINTER_COLOR[0],
                POINTER_COLOR[1],
                POINTER_COLOR[2],
                (a * 255.0) as u8,
            ));
            paint.anti_alias = true;
            frame.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }

        let pad = r * 2.2;
        Some((
            (cx - pad).max(0.0) as u32,
            (cy - pad).max(0.0) as u32,
            (cx + pad).min(theme.width as f32) as u32,
            (cy + pad).min(theme.height as f32) as u32,
        ))
    }
}

impl BoardRenderer {
    /// Render one frame, ready to hand to ffmpeg as yuv420p.
    pub fn render(&mut self, state: &ReplayState) -> &YuvFrame {
        let empty = Canvas {
            zoom: 1.0,
            ..Default::default()
        };
        let canvas = state.active().unwrap_or(&empty).clone();

        let (s, tx, ty) = transform_of(&canvas);
        let key = FrameKey {
            slide: state.current_slide,
            revision: canvas.revision,
            transform: (s.to_bits(), tx.to_bits(), ty.to_bits()),
            url: state.active_slide().and_then(|s| s.url.clone()),
        };

        let hit = matches!(&self.cached, Some((k, _)) if *k == key);
        if !hit {
            let mut board = self.background(state);
            self.draw_strokes(&mut board, &canvas);
            if let Some(zoomed) = self.apply_zoom(&board, &canvas) {
                board = zoomed;
            }
            // Convert the bare board once; every frame that shares it then
            // costs a memcpy instead of a full-frame colour conversion.
            yuv::convert(&board.as_ref(), &mut self.board_yuv);
            self.scratch.data_mut().copy_from_slice(board.data());
            self.cached = Some((key, board));
            self.out_yuv.copy_from(&self.board_yuv);
            self.dirty = None;
        }

        // The cursor moves independently of the board. Repaint only the
        // square it left behind and the square it now occupies, rather than
        // reconverting 921,600 pixels for a dot.
        let Self {
            cached,
            scratch,
            theme,
            board_yuv,
            out_yuv,
            dirty,
            ..
        } = self;
        let board = &cached.as_ref().expect("just populated").1;

        // Restore what the previous cursor covered.
        if let Some(rect) = dirty.take() {
            restore(scratch, board, rect, theme.width);
            yuv::convert_region(&scratch.as_ref(), out_yuv, rect);
        }
        let _ = board_yuv;

        if let Some(rect) = draw_pointer_into(scratch, &canvas, state.t_us, theme) {
            yuv::convert_region(&scratch.as_ref(), out_yuv, rect);
            *dirty = Some(rect);
        }
        out_yuv
    }
}

/// Copy a rectangle of the pristine board back over the scratch buffer.
fn restore(scratch: &mut Pixmap, board: &Pixmap, rect: (u32, u32, u32, u32), width: u32) {
    let (x0, y0, x1, y1) = rect;
    let w = width as usize;
    let dst = scratch.data_mut();
    let src = board.data();
    for y in y0 as usize..y1 as usize {
        let a = (y * w + x0 as usize) * 4;
        let b = (y * w + x1 as usize) * 4;
        if b <= src.len() {
            dst[a..b].copy_from_slice(&src[a..b]);
        }
    }
}

fn build_path(points: &[(f32, f32)], w: f32, h: f32) -> Option<tiny_skia::Path> {
    if points.len() < 2 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0 * w, points[0].1 * h);
    for p in &points[1..] {
        pb.line_to(p.0 * w, p.1 * h);
    }
    pb.finish()
}

/// Convert premultiplied RGBA to the packed RGB ffmpeg expects.
pub fn rgba_to_rgb(src: &PixmapRef, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(src.width() as usize * src.height() as usize * 3);
    for px in src.pixels() {
        // Pixels are premultiplied; demultiply so colours are not darkened.
        let a = px.alpha();
        if a == 255 {
            out.extend_from_slice(&[px.red(), px.green(), px.blue()]);
        } else if a == 0 {
            out.extend_from_slice(&[0, 0, 0]);
        } else {
            let f = 255.0 / a as f32;
            out.extend_from_slice(&[
                (px.red() as f32 * f).min(255.0) as u8,
                (px.green() as f32 * f).min(255.0) as u8,
                (px.blue() as f32 * f).min(255.0) as u8,
            ]);
        }
    }
}

/// A blank RGB frame, used when a board cannot be produced.
pub fn blank(width: u32, height: u32) -> Vec<u8> {
    vec![255u8; (width * height * 3) as usize]
}

#[allow(dead_code)]
fn unused(_: Rect) {}
