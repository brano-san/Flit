use flit_core::hash::FxHashMap;
use flit_core::ui::Bitmap;
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HEAD: u64 = 16;
const MAX_SIDE: u32 = 512;
const SEGMENTS: u64 = 8;
const MIN_SEGMENT: u64 = 1 << 20;

#[derive(Clone, Copy)]
struct Rec {
    seg: u32,
    at: u64,
    w: u32,
    h: u32,
}

pub struct ThumbCache {
    dir: PathBuf,
    limit: u64,
    index: FxHashMap<u64, Rec>,
    segs: VecDeque<(u32, u64)>,
    out: Option<File>,
    loaded: bool,
}

fn seg_path(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("{n:08}.thumbs"))
}

fn sane(w: u32, h: u32) -> bool {
    (1..=MAX_SIDE).contains(&w) && (1..=MAX_SIDE).contains(&h)
}

impl ThumbCache {
    pub fn new(dir: PathBuf, limit_mb: u32) -> Self {
        Self { dir, limit: (limit_mb as u64) << 20, index: FxHashMap::default(), segs: VecDeque::new(), out: None, loaded: false }
    }

    fn load(&mut self) {
        if std::mem::replace(&mut self.loaded, true) {
            return;
        }
        let mut nums: Vec<u32> = fs::read_dir(&self.dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".thumbs")?.parse().ok()).collect();
        nums.sort_unstable();
        for n in nums {
            let len = self.scan(n);
            self.segs.push_back((n, len));
        }
        self.trim();
    }

    fn scan(&mut self, n: u32) -> u64 {
        let Ok(f) = File::open(seg_path(&self.dir, n)) else { return 0 };
        let total = f.metadata().map_or(0, |m| m.len());
        let mut r = BufReader::new(f);
        let (mut at, mut head) = (0u64, [0u8; HEAD as usize]);
        while at + HEAD <= total && r.read_exact(&mut head).is_ok() {
            let word = |k: usize| u32::from_le_bytes(head[k..k + 4].try_into().unwrap());
            let (key, w, h) = (u64::from_le_bytes(head[..8].try_into().unwrap()), word(8), word(12));
            let len = w as u64 * h as u64 * 4;
            if !sane(w, h) || at + HEAD + len > total || r.seek_relative(len as i64).is_err() {
                break;
            }
            self.index.insert(key, Rec { seg: n, at, w, h });
            at += HEAD + len;
        }
        at
    }

    pub fn get(&mut self, key: u64) -> Option<Bitmap> {
        self.load();
        let rec = *self.index.get(&key)?;
        let read = || -> std::io::Result<Vec<u8>> {
            let mut f = File::open(seg_path(&self.dir, rec.seg))?;
            f.seek(SeekFrom::Start(rec.at + HEAD))?;
            let mut rgba = vec![0; (rec.w * rec.h * 4) as usize];
            f.read_exact(&mut rgba)?;
            Ok(rgba)
        };
        match read() {
            Ok(rgba) => Some(Bitmap { width: rec.w, height: rec.h, rgba }),
            Err(_) => {
                self.index.remove(&key);
                None
            }
        }
    }

    fn writer(&mut self) -> Option<(u32, u64)> {
        let seg_limit = (self.limit / SEGMENTS).max(MIN_SEGMENT);
        match self.segs.back().copied() {
            Some((n, len)) if len < seg_limit && self.out.is_some() => Some((n, len)),
            Some((n, len)) if len < seg_limit => self.open(n, len),
            last => {
                let n = last.map_or(1, |s| s.0 + 1);
                self.segs.push_back((n, 0));
                self.open(n, 0)
            }
        }
    }

    fn open(&mut self, n: u32, len: u64) -> Option<(u32, u64)> {
        fs::create_dir_all(&self.dir).ok()?;
        let mut f = OpenOptions::new().create(true).write(true).truncate(false).open(seg_path(&self.dir, n)).ok()?;
        if f.metadata().ok()?.len() != len {
            f.set_len(len).ok()?;
        }
        f.seek(SeekFrom::Start(len)).ok()?;
        self.out = Some(f);
        Some((n, len))
    }

    pub fn put(&mut self, key: u64, b: &Bitmap) {
        self.load();
        if !sane(b.width, b.height) || b.rgba.len() != (b.width * b.height * 4) as usize || self.index.contains_key(&key) {
            return;
        }
        let Some((n, at)) = self.writer() else { return };
        let mut rec = Vec::with_capacity(HEAD as usize + b.rgba.len());
        rec.extend(key.to_le_bytes());
        rec.extend(b.width.to_le_bytes());
        rec.extend(b.height.to_le_bytes());
        rec.extend(&b.rgba);
        if self.out.as_mut().is_none_or(|f| f.write_all(&rec).is_err()) {
            self.out = None;
            return;
        }
        self.index.insert(key, Rec { seg: n, at, w: b.width, h: b.height });
        if let Some(s) = self.segs.back_mut() {
            s.1 += rec.len() as u64;
        }
        self.trim();
    }

    fn trim(&mut self) {
        while self.segs.iter().map(|s| s.1).sum::<u64>() > self.limit && self.segs.len() > 1 {
            let Some((n, _)) = self.segs.pop_front() else { break };
            let _ = fs::remove_file(seg_path(&self.dir, n));
            self.index.retain(|_, r| r.seg != n);
        }
    }

    pub fn used(&mut self) -> u64 {
        self.load();
        self.segs.iter().map(|s| s.1).sum()
    }

    pub fn set_limit(&mut self, limit_mb: u32) {
        self.limit = (limit_mb as u64) << 20;
        if self.loaded {
            self.trim();
        }
    }

    pub fn clear(&mut self) {
        self.load();
        self.out = None;
        for (n, _) in self.segs.drain(..) {
            let _ = fs::remove_file(seg_path(&self.dir, n));
        }
        self.index.clear();
    }
}

#[cfg(test)]
#[path = "../test/thumbs.rs"]
mod tests;
