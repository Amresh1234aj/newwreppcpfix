//! The wire format of a class replay.
//!
//! A replay is three things: `output.webm` (the educator's webcam and audio),
//! `data.json` (every slide change and pen stroke, stamped in microseconds),
//! and slide images on a CDN. Only the first is a video file — everything you
//! actually look at during a class is drawn from `data.json`.

use anyhow::{anyhow, Result};
use serde_json::Value;

/// Plugin names as they appear in `data.json`.
pub const P_CANVAS: &str = "cw"; // drawing, pointer, zoom, pan
pub const P_DOC: &str = "dcn"; // slides, pen colour/mode/width
pub const P_MAIN: &str = "mcn"; // class-level events
pub const P_POLL: &str = "pl";
pub const P_CHAT: &str = "ch";

/// One record from `data.json`, normalised.
#[derive(Debug, Clone)]
pub struct Event {
    /// Microseconds since the class started. This is the master clock; it
    /// lines up with the webm's presentation timestamps.
    pub p_time: i64,
    pub plugin: String,
    pub kind: String,
    pub payload: Value,
    /// Canvas id. Canvas ids line up 1:1 with slide indices.
    pub canvas_id: Option<i64>,
    pub seq: i64,
}

impl Event {
    pub fn t(&self) -> f64 {
        self.p_time as f64 / 1_000_000.0
    }

    pub fn f(&self, key: &str) -> Option<f64> {
        self.payload.get(key).and_then(as_f64)
    }

    pub fn i(&self, key: &str) -> Option<i64> {
        self.payload.get(key).and_then(as_i64)
    }

    pub fn s(&self, key: &str) -> Option<&str> {
        self.payload.get(key).and_then(|v| v.as_str())
    }

    /// A normalised `{x, y}` point from the payload's `p` field.
    pub fn point(&self) -> Option<(f32, f32)> {
        let p = self.payload.get("p")?;
        let x = p.get("x").and_then(as_f64)?;
        let y = p.get("y").and_then(as_f64)?;
        Some((x as f32, y as f32))
    }
}

/// Numbers arrive as ints, floats, or float-shaped strings depending on the
/// class — `"2700081093.0"` is a real range key. Accept all three.
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s
            .parse::<i64>()
            .ok()
            .or_else(|| s.parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    }
}

/// Normalise one raw record.
///
/// The payload nests two different ways depending on the plugin:
///
/// ```text
/// {"data": {"data": {"e": "m", ...}, "id": 7}}   // canvas-scoped
/// {"data": {"e": "as", "i": 3, "u": "..."}}      // document-scoped
/// ```
fn parse_event(raw: &Value) -> Option<Event> {
    let body = raw.get("data")?;
    if !body.is_object() {
        return None;
    }

    let inner = body.get("data");
    let (payload, canvas_id) = match inner {
        Some(v) if v.is_object() && v.get("e").is_some() => (v, body.get("id").and_then(as_i64)),
        _ => (body, body.get("id").and_then(as_i64)),
    };

    let kind = payload.get("e")?.as_str()?.to_string();

    Some(Event {
        p_time: raw.get("p_time").and_then(as_i64).unwrap_or(0),
        plugin: raw
            .get("plugin")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        kind,
        payload: payload.clone(),
        canvas_id,
        seq: raw.get("id").and_then(as_i64).unwrap_or(0),
    })
}

/// Parse a whole `data.json`: `[[event, ...], [event, ...], ...]`.
pub fn load_events(text: &str) -> Result<Vec<Event>> {
    let doc: Value = serde_json::from_str(text)?;
    let chunks = doc
        .as_array()
        .ok_or_else(|| anyhow!("data.json is not an array"))?;

    let mut events = Vec::with_capacity(32_000);
    for chunk in chunks {
        // The file is a list of chunks, but a single chunk may be handed in
        // on its own when seeking by byte range.
        match chunk.as_array() {
            Some(list) => events.extend(list.iter().filter_map(parse_event)),
            None => {
                if let Some(ev) = parse_event(chunk) {
                    events.push(ev)
                }
            }
        }
    }
    events.sort_by_key(|e| (e.p_time, e.seq));
    Ok(events)
}

/// What we need to locate and label one class.
#[derive(Debug, Clone, Default)]
pub struct ClassInfo {
    pub class_uid: String,
    /// The CDN folder name — not the class uid.
    pub media_uid: String,
    pub title: String,
    pub educator: String,
    pub duration: f64,
    pub webm_file: String,
    pub media_host: String,
    pub data_host: String,
    pub slides_pdf: Option<String>,
}

impl ClassInfo {
    pub fn data_url(&self) -> String {
        format!(
            "https://{}/lesson-raw/{}/data.json",
            self.data_host, self.media_uid
        )
    }

    pub fn media_url(&self) -> String {
        format!(
            "https://{}/lesson-raw/{}/{}",
            self.media_host, self.media_uid, self.webm_file
        )
    }
}
