use crate::i18n::S;
use serde_json::{json, Map, Value as Json};
use std::cmp::Ordering;

pub const MANIFEST_URL: &str = "https://github.com/brano-san/Flit/releases/latest/download/latest.json";
pub const RELEASES_URL: &str = "https://github.com/brano-san/Flit/releases/latest";
pub const WINDOWS: &str = "windows-x86_64";
pub const LINUX: &str = "linux-x86_64";
pub const PLATFORM: &str = if cfg!(windows) { WINDOWS } else { LINUX };
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateRequest {
    Check { manual: bool },
    Install(Release),
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateEvent {
    Checked { manual: bool, found: Result<Option<Release>, String> },
    Progress { received: u64, total: Option<u64> },
    Installing,
    Failed(String),
    ReadOnly(String),
}

pub fn parse_manifest(text: &str, platform: &str) -> Result<Release, String> {
    let bad = || S::UpdateErrorManifest.t().to_owned();
    let doc: Json = serde_json::from_str(text).map_err(|_| bad())?;
    let version = doc["version"].as_str().map(|v| v.trim().trim_start_matches('v')).filter(|v| parse_version(v).is_some()).ok_or_else(bad)?;
    let entry = &doc["platforms"][platform];
    let field = |name: &str| entry[name].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let (Some(url), Some(signature)) = (field("url"), field("signature")) else {
        return Err(crate::tr!(S::UpdateErrorNoBuild, version = version, platform = platform));
    };
    if !url.starts_with("https://") || base64_decode(&signature).is_none() {
        return Err(bad());
    }
    let notes = doc["notes"].as_str().unwrap_or("").trim().to_owned();
    Ok(Release { version: version.to_owned(), notes, url, signature })
}

pub fn render_manifest(version: &str, notes: &str, pub_date: &str, platforms: &[(&str, String, String)]) -> String {
    let mut map = Map::new();
    for (platform, url, signature) in platforms {
        map.insert((*platform).to_owned(), json!({ "url": url, "signature": signature }));
    }
    let doc = json!({ "version": version, "notes": notes, "pub_date": pub_date, "platforms": map });
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

pub fn platform_of_asset(name: &str) -> Option<&'static str> {
    if name.ends_with("-x86_64-windows.zip") {
        Some(WINDOWS)
    } else if name.ends_with("-x86_64-linux.tar.gz") {
        Some(LINUX)
    } else {
        None
    }
}

fn parse_version(v: &str) -> Option<(Vec<u64>, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once(['-', '+']) {
        Some((core, rest)) => (core, v[core.len()..].starts_with('-') && !rest.is_empty()),
        None => (v, false),
    };
    let parts = core.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
    (!parts.is_empty()).then_some((parts, pre))
}

pub fn newer(candidate: &str, current: &str) -> bool {
    let (Some((a, a_pre)), Some((b, b_pre))) = (parse_version(candidate), parse_version(current)) else { return false };
    let n = a.len().max(b.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    match (0..n).map(|i| at(&a, i).cmp(&at(&b, i))).find(|o| *o != Ordering::Equal) {
        Some(o) => o == Ordering::Greater,
        None => b_pre && !a_pre,
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim().trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = ALPHABET.iter().position(|&a| a == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (acc == 0 && bits < 6).then_some(out)
}

pub fn percent(received: u64, total: Option<u64>) -> Option<u8> {
    total.filter(|&t| t > 0).map(|t| (received.min(t) * 100 / t) as u8)
}

#[cfg(test)]
#[path = "../test/update.rs"]
mod tests;
