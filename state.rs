//! The replay state machine.
//!
//! Feed it events in `p_time` order and it reconstructs what the browser
//! player shows: which slide is up, every stroke on every canvas, the pen
//! state, zoom/pan, and the cursor.
//!
//! Event vocabulary, decoded from the player bundle:
//!
//! ```text
//! cw  d/m/u  pen down / move / up, points normalised to [0, 1]
//! cw  p      pointer — the cursor, no ink
//! cw  ea     erase everything on that canvas
//! cw  zm/pn  zoom factor / pan offset
//! dcn as/sc  add slide {i, u, bc} / show slide {s}
//! dcn cc/mc  pen colour / mode (marker, highlighter, eraser)
//! dcn pstc   pen thickness      estc  eraser thickness
//! mcn cle    class ended
//! ```

use std::collections::HashMap;

use crate::model::{as_f64, Event, P_CANVAS, P_DOC, P_MAIN};

pub const MARKER: &str = "marker";
pub const HIGHLIGHTER: &str = "highlighter";
pub const ERASER: &str = "eraser";
pub const DEFAULT_COLOR: &str = "#2C3844";

#[derive(Debug, Clone)]
pub struct Stroke {
    pub mode: String,
    pub color: [u8; 3],
    pub width: f32,
    pub points: Vec<(f32, f32)>,
}

impl Stroke {
    pub fn is_erase(&self) -> bool {
        self.mode == ERASER
    }
    pub fn is_highlight(&self) -> bool {
        self.mode == HIGHLIGHTER
    }
}

/// One board. Canvas ids line up 1:1 with slide indices.
#[derive(Debug, Clone, Default)]
pub struct Canvas {
    pub strokes: Vec<Stroke>,
    pub open: Option<Stroke>,
    pub zoom: f32,
    pub pan: (f32, f32),
    pub pointer: Option<(f32, f32)>,
    pub pointer_us: i64,
    /// Bumped on every visible change, so renderers can cache by revision.
    pub revision: u64,
}

impl Canvas {
    fn new() -> Self {
        Canvas {
            zoom: 1.0,
            ..Default::default()
        }
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    /// Opacity of the laser dot: solid, then fading, then gone.
    pub fn pointer_alpha(&self, t_us: i64) -> f32 {
        const HOLD: f32 = 1.0;
        const FADE: f32 = 0.8;
        if self.pointer.is_none() {
            return 0.0;
        }
        let age = (t_us - self.pointer_us) as f32 / 1_000_000.0;
        if age <= HOLD {
            1.0
        } else if age >= HOLD + FADE {
            0.0
        } else {
            1.0 - (age - HOLD) / FADE
        }
    }

    pub fn visible_strokes(&self) -> impl Iterator<Item = &Stroke> {
        self.strokes.iter().chain(
            self.open
                .iter()
                .filter(|s| s.points.len() > 1),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Slide {
    pub url: Option<String>,
    pub background: [u8; 3],
}

pub struct ReplayState {
    pub slides: HashMap<i64, Slide>,
    pub canvases: HashMap<i64, Canvas>,
    pub current_slide: i64,
    pub color: [u8; 3],
    pub mode: String,
    pub pen_scale: f32,
    pub eraser_scale: f32,
    pub t_us: i64,
    applied: usize,
}

impl Default for ReplayState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayState {
    pub fn new() -> Self {
        ReplayState {
            slides: HashMap::new(),
            canvases: HashMap::new(),
            current_slide: 0,
            color: hex_to_rgb(DEFAULT_COLOR),
            mode: MARKER.to_string(),
            pen_scale: 1.0,
            eraser_scale: 1.0,
            t_us: 0,
            applied: 0,
        }
    }

    pub fn canvas(&mut self, cid: i64) -> &mut Canvas {
        self.canvases.entry(cid).or_insert_with(Canvas::new)
    }

    pub fn active(&self) -> Option<&Canvas> {
        self.canvases.get(&self.current_slide)
    }

    pub fn active_slide(&self) -> Option<&Slide> {
        self.slides.get(&self.current_slide)
    }

    pub fn slide_count(&self) -> i64 {
        self.slides.keys().copied().max().unwrap_or(0)
    }

    /// Apply every not-yet-applied event up to `t_us`. Forward-only.
    pub fn advance(&mut self, events: &[Event], t_us: i64) {
        let mut i = self.applied;
        while i < events.len() && events[i].p_time <= t_us {
            self.apply(&events[i]);
            i += 1;
        }
        self.applied = i;
        self.t_us = t_us;
    }

    fn target(&mut self, ev: &Event) -> i64 {
        ev.canvas_id.unwrap_or(self.current_slide)
    }

    fn apply(&mut self, ev: &Event) {
        self.t_us = ev.p_time;
        match (ev.plugin.as_str(), ev.kind.as_str()) {
            (P_CANVAS, "d") => self.pen_down(ev),
            (P_CANVAS, "m") => self.pen_move(ev),
            (P_CANVAS, "u") => self.pen_up(ev),
            (P_CANVAS, "p") => self.pointer(ev),
            (P_CANVAS, "ea") => {
                let c = self.target(ev);
                let cv = self.canvas(c);
                cv.strokes.clear();
                cv.open = None;
                cv.touch();
            }
            (P_CANVAS, "zm") => {
                let c = self.target(ev);
                let v = ev.f("v").unwrap_or(1.0) as f32;
                let cv = self.canvas(c);
                cv.zoom = v.max(1.0);
                cv.touch();
            }
            (P_CANVAS, "pn") => {
                let c = self.target(ev);
                if let Some(v) = ev.payload.get("v") {
                    let x = v.get("x").and_then(as_f64).unwrap_or(0.0) as f32;
                    let y = v.get("y").and_then(as_f64).unwrap_or(0.0) as f32;
                    let cv = self.canvas(c);
                    cv.pan = (x, y);
                    cv.touch();
                }
            }
            (P_DOC, "as") => {
                if let Some(i) = ev.i("i") {
                    self.slides.insert(
                        i,
                        Slide {
                            url: ev.s("u").map(str::to_string),
                            background: ev.s("bc").map(hex_to_rgb).unwrap_or([255, 255, 255]),
                        },
                    );
                }
            }
            (P_DOC, "sc") => {
                if let Some(s) = ev.i("s") {
                    self.current_slide = s;
                }
            }
            (P_DOC, "cc") => {
                if let Some(c) = ev.s("c") {
                    self.color = hex_to_rgb(c);
                }
            }
            (P_DOC, "mc") => {
                if let Some(m) = ev.s("m") {
                    self.mode = m.to_string();
                }
            }
            (P_DOC, "pstc") => {
                if let Some(s) = ev.f("s") {
                    self.pen_scale = s as f32;
                }
            }
            (P_DOC, "estc") => {
                if let Some(s) = ev.f("s") {
                    self.eraser_scale = s as f32;
                }
            }
            (P_MAIN, "cle") => {}
            _ => {}
        }
    }

    fn pen_down(&mut self, ev: &Event) {
        let Some(pt) = ev.point() else { return };
        let cid = self.target(ev);
        let mode = ev
            .payload
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.mode)
            .to_string();
        let width = if mode == ERASER {
            self.eraser_scale
        } else {
            self.pen_scale
        };
        let color = self.color;
        let p_time = ev.p_time;
        let cv = self.canvas(cid);
        cv.open = Some(Stroke {
            mode,
            color,
            width,
            points: vec![pt],
        });
        cv.pointer = Some(pt);
        cv.pointer_us = p_time;
        cv.touch();
    }

    fn pen_move(&mut self, ev: &Event) {
        let Some(pt) = ev.point() else { return };
        let cid = self.target(ev);
        let p_time = ev.p_time;
        let cv = self.canvas(cid);
        cv.pointer = Some(pt);
        cv.pointer_us = p_time;
        if let Some(open) = cv.open.as_mut() {
            open.points.push(pt);
            cv.revision += 1;
        }
    }

    fn pen_up(&mut self, ev: &Event) {
        let pt = ev.point();
        let cid = self.target(ev);
        let p_time = ev.p_time;
        let cv = self.canvas(cid);
        if let Some(mut open) = cv.open.take() {
            if let Some(p) = pt {
                open.points.push(p);
            }
            if open.points.len() == 1 {
                // A tap still leaves a dot.
                open.points.push(open.points[0]);
            }
            cv.strokes.push(open);
            cv.touch();
        }
        if let Some(p) = pt {
            cv.pointer = Some(p);
            cv.pointer_us = p_time;
        }
    }

    fn pointer(&mut self, ev: &Event) {
        let Some(pt) = ev.point() else { return };
        let cid = self.target(ev);
        let p_time = ev.p_time;
        let cv = self.canvas(cid);
        cv.pointer = Some(pt);
        cv.pointer_us = p_time;
    }
}

/// Replay from the top up to `t_us`.
pub fn build_at(events: &[Event], t_us: i64) -> ReplayState {
    let mut st = ReplayState::new();
    st.advance(events, t_us);
    st
}

pub fn hex_to_rgb(value: &str) -> [u8; 3] {
    let v = value.trim_start_matches('#');
    let v = if v.len() == 3 {
        v.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        v.to_string()
    };
    if v.len() != 6 {
        return [44, 56, 68];
    }
    let p = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).unwrap_or(0);
    [p(0), p(2), p(4)]
}
