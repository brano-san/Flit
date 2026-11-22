pub const MAX_EXTS: usize = 64;
pub const TICKS_PER_SEC: i64 = 10_000_000;
pub const UNIX_EPOCH_SECS: i64 = 11_644_473_600;
pub const DAY: i64 = 86_400;

#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now: i64,
    pub utc_offset_min: i32,
}

impl Clock {
    pub fn system(utc_offset_min: i32) -> Self {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        Self { now: (secs + UNIX_EPOCH_SECS) * TICKS_PER_SEC, utc_offset_min }
    }

    fn today(&self) -> i64 {
        (self.now / TICKS_PER_SEC - UNIX_EPOCH_SECS + self.utc_offset_min as i64 * 60).div_euclid(DAY)
    }

    fn day_start(&self, unix_day: i64) -> i64 {
        (unix_day * DAY - self.utc_offset_min as i64 * 60 + UNIX_EPOCH_SECS) * TICKS_PER_SEC
    }
}

pub fn extensions(value: &str) -> Option<Vec<Vec<u8>>> {
    (value.split(';').count() <= MAX_EXTS).then(|| value.split(';').map(|e| {
        e.trim_start_matches('.').chars().flat_map(char::to_lowercase).collect::<String>().into_bytes()
    }).collect())
}

pub fn size_range(value: &str) -> Option<(u64, u64)> {
    let (lo, hi) = interval(value, &|x| size_value(x).map(|n| (n as i128, n as i128 + 1)))?;
    Some((lo.clamp(0, u64::MAX as i128) as u64, hi.clamp(0, u64::MAX as i128) as u64))
}

pub fn date_range(value: &str, clock: Clock) -> Option<(i64, i64)> {
    let (lo, hi) = interval(value, &|x| date_value(x, clock))?;
    Some((lo.clamp(i64::MIN as i128, i64::MAX as i128) as i64, hi.clamp(i64::MIN as i128, i64::MAX as i128) as i64))
}

pub fn number_range(value: &str) -> Option<(u64, u64)> {
    let (lo, hi) = interval(value, &|x| x.parse::<u32>().ok().map(|n| (n as i128, n as i128 + 1)))?;
    Some((lo.max(0) as u64, hi.clamp(0, u64::MAX as i128) as u64))
}

pub fn interval(v: &str, value: &dyn Fn(&str) -> Option<(i128, i128)>) -> Option<(i128, i128)> {
    let (open_lo, open_hi) = (i128::MIN, i128::MAX);
    if let Some(x) = v.strip_prefix(">=") {
        return Some((value(x)?.0, open_hi));
    }
    if let Some(x) = v.strip_prefix("<=") {
        return Some((open_lo, value(x)?.1));
    }
    if let Some(x) = v.strip_prefix('>') {
        return Some((value(x)?.1, open_hi));
    }
    if let Some(x) = v.strip_prefix('<') {
        return Some((open_lo, value(x)?.0));
    }
    let v = v.strip_prefix('=').unwrap_or(v);
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.is_empty() { open_lo } else { value(a)?.0 };
        let hi = if b.is_empty() { open_hi } else { value(b)?.1 };
        return Some((lo, hi));
    }
    value(v)
}

fn size_value(x: &str) -> Option<u64> {
    let x = x.trim().to_ascii_lowercase();
    let split = x.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(x.len());
    let (num, unit) = x.split_at(split);
    let mult: u64 = match unit {
        "" | "b" => 1,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        _ => return None,
    };
    let n: f64 = num.parse().ok().filter(|n: &f64| n.is_finite() && *n >= 0.0)?;
    Some((n * mult as f64).round().min(u64::MAX as f64) as u64)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_value(x: &str, clock: Clock) -> Option<(i128, i128)> {
    let x = x.trim().to_ascii_lowercase();
    let today = clock.today();
    let (first, last) = match x.as_str() {
        "today" => (today, today + 1),
        "yesterday" => (today - 1, today),
        "week" => (today - 6, today + 1),
        "month" => (today - 29, today + 1),
        "year" => (today - 364, today + 1),
        _ => {
            let parts: Vec<&str> = x.split(['-', '/']).collect();
            let num = |s: &str| s.parse::<i64>().ok().filter(|_| !s.is_empty() && s.len() <= 4);
            match parts.as_slice() {
                [y] if y.len() == 4 => {
                    let y = num(y)?;
                    (days_from_civil(y, 1, 1), days_from_civil(y + 1, 1, 1))
                }
                [y, m] if y.len() == 4 => {
                    let (y, m) = (num(y)?, num(m)?);
                    if !(1..=12).contains(&m) {
                        return None;
                    }
                    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                    (days_from_civil(y, m as u32, 1), days_from_civil(ny, nm as u32, 1))
                }
                [y, m, d] if y.len() == 4 => {
                    let (y, m, d) = (num(y)?, num(m)?, num(d)?);
                    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
                        return None;
                    }
                    let start = days_from_civil(y, m as u32, d as u32);
                    (start, start + 1)
                }
                _ => return None,
            }
        }
    };
    Some((clock.day_start(first) as i128, clock.day_start(last) as i128))
}


#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchPaths {
    pub exclude: Vec<crate::FsPath>,
    pub parent: Option<crate::FsPath>,
    pub under: Option<crate::FsPath>,
}

impl SearchPaths {
    pub fn is_empty(&self) -> bool {
        self.exclude.is_empty() && self.parent.is_none() && self.under.is_none()
    }

    pub fn rejects(&self, path: &[u8]) -> bool {
        if self.under.as_ref().is_some_and(|folder| !Self::has_prefix(path, folder, false)) {
            return true;
        }
        if let Some(parent) = &self.parent {
            let sep = parent.sep();
            let Some(at) = path.iter().rposition(|&b| b == sep) else { return true };
            let actual = path[..at].strip_suffix(&[sep]).unwrap_or(&path[..at]);
            let expected = parent.as_bytes().strip_suffix(&[sep]).unwrap_or(parent.as_bytes());
            let same = if parent.case_sensitive() { actual == expected } else { actual.eq_ignore_ascii_case(expected) };
            if !same {
                return true;
            }
        }
        self.excludes_branch(path)
    }

    pub fn excludes_branch(&self, path: &[u8]) -> bool {
        self.exclude.iter().any(|excluded| Self::has_prefix(path, excluded, true))
    }

    fn has_prefix(path: &[u8], folder: &crate::FsPath, allow_same: bool) -> bool {
        let prefix = folder.as_bytes();
        path.get(..prefix.len()).is_some_and(|head| {
            let same = if folder.case_sensitive() { head == prefix } else { head.eq_ignore_ascii_case(prefix) };
            same && if path.len() == prefix.len() { allow_same } else { folder.is_root() || path[prefix.len()] == folder.sep() }
        })
    }
}

pub type FolderCounts = Vec<(Vec<u8>, u64)>;

pub fn finish_folder_counts(mut counts: FolderCounts) -> FolderCounts {
    counts.sort_unstable_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    counts.truncate(256);
    counts
}

pub const SIZE_STARTS: [u64; 5] = [0, 1 << 10, 1 << 20, 100 << 20, 1 << 30];

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Facets {
    pub extensions: Vec<(String, u64)>,
    pub years: Vec<(i64, u64)>,
    pub sizes: [u64; 5],
    pub dates: Vec<DateBucket>,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DateBucket {
    pub from: i64,
    pub to: i64,
    pub count: u64,
}

#[derive(Default)]
pub struct FacetCounts {
    ext: std::collections::HashMap<String, u64>,
    days: std::collections::BTreeMap<i64, u64>,
    sizes: [u64; 5],
}

impl FacetCounts {
    pub fn add(&mut self, name: &[u8], meta: crate::Meta, offset: i32) {
        if meta.attrs & crate::attr::DIR != 0 {
            *self.ext.entry("/".into()).or_default() += 1;
        } else {
            let ext = &name[name.len() - crate::name::ext_len(name)..];
            let ext = String::from_utf8_lossy(ext);
            if ext.len() <= 32 && !ext.contains(['"', ';']) {
                *self.ext.entry(ext.to_lowercase()).or_default() += 1;
            }
            let bucket = SIZE_STARTS.partition_point(|&v| v <= meta.size) - 1;
            self.sizes[bucket] += 1;
        }
        if meta.modified > 0 {
            let day = crate::fmt::local_days(meta.modified, offset);
            *self.days.entry(day).or_default() += 1;
        }
    }

    pub fn finish(self) -> Facets {
        let mut extensions: Vec<_> = self.ext.into_iter().collect();
        extensions.sort_unstable_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
        extensions.truncate(128);
        let mut dates = Vec::new();
        let mut years = std::collections::BTreeMap::<i64, u64>::new();
        if let Some((&first, _)) = self.days.first_key_value() {
            let last = *self.days.last_key_value().unwrap().0;
            let span = last - first + 1;
            let step = (span + 47) / 48;
            for i in 0..((span + step - 1) / step) {
                let from = first + i * step;
                dates.push(DateBucket { from, to: (from + step - 1).min(last), count: 0 });
            }
            for (day, n) in self.days {
                dates[((day - first) / step) as usize].count += n;
                *years.entry(crate::fmt::civil_from_days(day).0).or_default() += n;
            }
        }
        let years = years.into_iter().rev().take(512).collect();
        Facets { extensions, years, sizes: self.sizes, dates }
    }
}
