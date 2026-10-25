use std::fmt::Write;

pub fn size(out: &mut impl Write, bytes: u64) {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        let _ = write!(out, "{bytes} B");
        return;
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let _ = if v < 100.0 { write!(out, "{v:.1} {}", UNITS[u]) } else { write!(out, "{v:.0} {}", UNITS[u]) };
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
