use std::fmt::Write;

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DateFormat {
    #[default]
    Universal,
    System,
    Application,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Style {
    pub dates: DateFormat,
    pub decimal: bool,
    pub system: (String, String),
    pub today: Option<i64>,
}

pub fn size(out: &mut impl Write, bytes: u64) {
    size_in(out, bytes, false);
}

pub fn size_in(out: &mut impl Write, bytes: u64, decimal: bool) {
    let (base, units) = if decimal { (1000.0, ["B", "kB", "MB", "GB", "TB", "PB"]) } else { (1024.0, ["B", "KB", "MB", "GB", "TB", "PB"]) };
    if (bytes as f64) < base {
        let _ = write!(out, "{bytes} B");
        return;
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= base && u < units.len() - 1 {
        v /= base;
        u += 1;
    }
    let _ = if v < 100.0 { write!(out, "{v:.1} {}", units[u]) } else { write!(out, "{v:.0} {}", units[u]) };
}

fn local_secs(ticks: i64, offset_min: i32) -> i64 {
    ticks / 10_000_000 - 11_644_473_600 + offset_min as i64 * 60
}

pub fn local_days(ticks: i64, offset_min: i32) -> i64 {
    local_secs(ticks, offset_min).div_euclid(86_400)
}

pub fn datetime(out: &mut impl Write, ticks: i64, offset_min: i32) {
    if ticks <= 0 {
        return;
    }
    let secs = local_secs(ticks, offset_min);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let _ = write!(out, "{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60);
}

pub fn datetime_in(out: &mut impl Write, ticks: i64, offset_min: i32, style: &Style) {
    if ticks <= 0 {
        return;
    }
    match style.dates {
        DateFormat::Universal => datetime(out, ticks, offset_min),
        DateFormat::System => {
            let secs = local_secs(ticks, offset_min);
            picture(out, &style.system.0, secs);
            let _ = out.write_char(' ');
            picture(out, &style.system.1, secs);
        }
        DateFormat::Application => friendly(out, local_secs(ticks, offset_min), style.today),
    }
}

fn friendly(out: &mut impl Write, secs: i64, today: Option<i64>) {
    let day = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm) = (rem / 3600, rem % 3600 / 60);
    let (y, m, d) = civil_from_days(day);
    let month = &MONTHS[m as usize - 1][..3];
    let _ = match today.map(|t| t - day) {
        Some(0) => write!(out, "Today {hh:02}:{mm:02}"),
        Some(1) => write!(out, "Yesterday {hh:02}:{mm:02}"),
        Some(2..=6) => write!(out, "{} {hh:02}:{mm:02}", DAYS[(day + 4).rem_euclid(7) as usize]),
        Some(_) if today.is_some_and(|t| civil_from_days(t).0 == y) => write!(out, "{d} {month} {hh:02}:{mm:02}"),
        _ => write!(out, "{d} {month} {y}"),
    };
}

pub fn picture(out: &mut impl Write, pattern: &str, secs: i64) {
    let day = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(day);
    let (h, min, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' && chars.get(i + 1) == Some(&'\'') {
                    let _ = out.write_char('\'');
                    i += 2;
                } else if chars[i] == '\'' {
                    break;
                } else {
                    let _ = out.write_char(chars[i]);
                    i += 1;
                }
            }
            i += 1;
            continue;
        }
        let n = chars[i..].iter().take_while(|&&x| x == c).count();
        i += n;
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        let _ = match (c, n) {
            ('d', 1) => write!(out, "{d}"),
            ('d', 2) => write!(out, "{d:02}"),
            ('d', 3) => out.write_str(&DAYS[(day + 4).rem_euclid(7) as usize][..3]),
            ('d', _) => out.write_str(DAYS[(day + 4).rem_euclid(7) as usize]),
            ('M', 1) => write!(out, "{m}"),
            ('M', 2) => write!(out, "{m:02}"),
            ('M', 3) => out.write_str(&MONTHS[m as usize - 1][..3]),
            ('M', _) => out.write_str(MONTHS[m as usize - 1]),
            ('y', 1) => write!(out, "{}", y.rem_euclid(100)),
            ('y', 2) => write!(out, "{:02}", y.rem_euclid(100)),
            ('y', _) => write!(out, "{y:04}"),
            ('h', 1) => write!(out, "{h12}"),
            ('h', _) => write!(out, "{h12:02}"),
            ('H', 1) => write!(out, "{h}"),
            ('H', _) => write!(out, "{h:02}"),
            ('m', 1) => write!(out, "{min}"),
            ('m', _) => write!(out, "{min:02}"),
            ('s', 1) => write!(out, "{s}"),
            ('s', _) => write!(out, "{s:02}"),
            ('t', 1) => out.write_str(if h < 12 { "A" } else { "P" }),
            ('t', _) => out.write_str(if h < 12 { "AM" } else { "PM" }),
            ('g', _) => Ok(()),
            _ => (0..n).try_for_each(|_| out.write_char(c)),
        };
    }
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

#[cfg(test)]
#[path = "../test/fmt.rs"]
mod tests;
