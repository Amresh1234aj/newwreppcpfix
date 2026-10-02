//! The three calls that locate a class.
//!
//! ```text
//! api.unacademy.com/v1/uplus/classes/<uid>/details/
//!     -> video_url=...?uid=<media_uid>   the CDN folder, not the class uid
//!     -> slides_pdf.with_annotation
//!
//! unacademy.com/api/v1/uplus/classes/<uid>/replay_token/?include_meta_json=1
//!     -> meta_json.range   chunk byte spans; the last key is the class length
//!     -> domain_config     which CDN hosts to use
//!
//! <data_host>/lesson-raw/<media_uid>/data.json    every board event
//! ```
//!
//! Auth is a bearer JWT — the same value as the `accessToken` cookie.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::assets::{PLAYER_ORIGIN, UA};
use crate::model::{as_f64, ClassInfo};

const API: &str = "https://api.unacademy.com";
const WEB: &str = "https://unacademy.com";

pub struct Client {
    pub agent: Arc<ureq::Agent>,
    token: String,
}

impl Client {
    pub fn new(token: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(120))
            .user_agent(UA)
            .build();
        Client {
            agent: Arc::new(agent),
            token: token.trim().to_string(),
        }
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self
            .agent
            .get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/json, text/plain, */*")
            .set("Origin", PLAYER_ORIGIN)
            .set("Referer", &format!("{PLAYER_ORIGIN}/"))
            .call();

        match resp {
            Ok(r) => Ok(r.into_json()?),
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => bail!(
                "{code} from the API — the token is expired or the account \
                 cannot access this class. Grab a fresh accessToken cookie."
            ),
            Err(e) => Err(anyhow!(e)).context(format!("requesting {url}")),
        }
    }

    pub fn get_text(&self, url: &str) -> Result<String> {
        Ok(self
            .agent
            .get(url)
            .set("Origin", PLAYER_ORIGIN)
            .set("Referer", &format!("{PLAYER_ORIGIN}/"))
            .call()?
            .into_string()?)
    }

    pub fn class_info(&self, url_or_uid: &str) -> Result<ClassInfo> {
        let uid = parse_class_uid(url_or_uid)?;
        let details = self.get_json(&format!("{API}/v1/uplus/classes/{uid}/details/"))?;
        let replay = self.get_json(&format!(
            "{WEB}/api/v1/uplus/classes/{uid}/replay_token/?include_meta_json=1"
        ))?;
        build_class_info(&uid, &details, &replay)
    }
}

pub fn parse_class_uid(input: &str) -> Result<String> {
    let s = input.trim();

    // .../class/<slug>/<UID> or .../lesson/<slug>/<UID>
    for marker in ["/class/", "/lesson/"] {
        if let Some(rest) = s.split(marker).nth(1) {
            if let Some(uid) = rest.split(['/', '?', '#']).nth(1) {
                if is_uid(uid) {
                    return Ok(uid.to_uppercase());
                }
            }
        }
    }
    // ...?liveclass=<UID>
    if let Some(rest) = s.split("liveclass=").nth(1) {
        let uid = rest.split(['&', '#']).next().unwrap_or("");
        if is_uid(uid) {
            return Ok(uid.to_uppercase());
        }
    }
    if is_uid(s) {
        return Ok(s.to_uppercase());
    }
    bail!("could not find a class uid in {input:?}")
}

fn is_uid(s: &str) -> bool {
    s.len() >= 6 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

fn build_class_info(uid: &str, details: &Value, replay: &Value) -> Result<ClassInfo> {
    let video_url = details
        .get("video_url")
        .or_else(|| details.get("videoUrl"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let media_uid = video_url
        .split("uid=")
        .nth(1)
        .and_then(|r| r.split(['&', '#']).next())
        .filter(|s| s.len() >= 8)
        .ok_or_else(|| {
            anyhow!("no media uid in the class details — the class may have no replay yet")
        })?
        .to_string();

    let cfg = replay.get("domain_config");
    let host = |key: &str, fallback: &str| -> String {
        cfg.and_then(|c| c.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or(fallback)
            .to_string()
    };

    let meta = replay.get("meta_json");

    // The last range key is the p_time of the final event, in microseconds —
    // a more accurate length than the API's own duration field. Some classes
    // write these keys as floats ("2700081093.0").
    let duration = meta
        .and_then(|m| m.get("range"))
        .and_then(|r| r.as_object())
        .map(|map| {
            map.keys()
                .filter_map(|k| k.parse::<f64>().ok())
                .fold(0.0f64, f64::max)
                / 1e6
        })
        .filter(|d| *d > 0.0)
        .or_else(|| details.get("video_duration").and_then(as_f64))
        .unwrap_or(0.0);

    let webm_file = meta
        .and_then(|m| m.get("webm_video_file"))
        .and_then(|f| f.get("file"))
        .and_then(|v| v.as_str())
        .unwrap_or("output.webm")
        .to_string();

    let author = details.get("author");
    let name_of = |key: &str| -> String {
        author
            .and_then(|a| a.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let educator = {
        let full = name_of("name");
        if full.is_empty() {
            format!("{} {}", name_of("first_name"), name_of("last_name"))
                .trim()
                .to_string()
        } else {
            full
        }
    };

    Ok(ClassInfo {
        class_uid: uid.to_string(),
        media_uid,
        title: details
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(uid)
            .to_string(),
        educator,
        duration,
        webm_file,
        media_host: host("media_domain", "uamedia.uacdn.net"),
        data_host: host("data_domain", "player.uacdn.net"),
        slides_pdf: details
            .get("slides_pdf")
            .and_then(|s| s.get("with_annotation"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}
