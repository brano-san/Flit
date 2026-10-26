use flit_core::i18n::S;
use flit_core::preview::PropGroup;
use flit_platform::Platform;
use std::ffi::CStr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn mode_text(mode: u32) -> String {
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    let exec = |x: u32, special: u32, on: char, off: char| match (mode & x != 0, mode & special != 0) {
        (true, true) => on,
        (false, true) => off,
        (true, false) => 'x',
        (false, false) => '-',
    };
    let s: String = [
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        exec(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        exec(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        exec(0o001, 0o1000, 't', 'T'),
    ]
    .iter()
    .collect();
    format!("{s} ({:o})", mode & 0o7777)
}

fn name_of(id: u32, user: bool) -> String {
    let mut buf = vec![0 as libc::c_char; 4096];
    let found = unsafe {
        if user {
            let mut pw: libc::passwd = std::mem::zeroed();
            let mut out = std::ptr::null_mut();
            (libc::getpwuid_r(id, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null()).then(|| CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned())
        } else {
            let mut gr: libc::group = std::mem::zeroed();
            let mut out = std::ptr::null_mut();
            (libc::getgrgid_r(id, &mut gr, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null()).then(|| CStr::from_ptr(gr.gr_name).to_string_lossy().into_owned())
        }
    };
    match found {
        Some(name) => format!("{name} ({id})"),
        None => id.to_string(),
    }
}

fn date(t: SystemTime, offset: i32) -> Option<String> {
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    let ticks = d.as_secs() as i64 * 10_000_000 + (d.subsec_nanos() / 100) as i64 + 116_444_736_000_000_000;
    let mut s = String::new();
    flit_core::fmt::datetime(&mut s, ticks, offset);
    Some(s)
}

pub fn properties(path: &Path) -> Vec<PropGroup> {
    let Ok(m) = std::fs::symlink_metadata(path) else { return Vec::new() };
    let access = PropGroup {
        title: S::PropertiesAccess.t().to_owned(),
        rows: vec![
            (S::PropertiesPermissions.t().to_owned(), mode_text(m.mode())),
            (S::PropertiesOwner.t().to_owned(), name_of(m.uid(), true)),
            (S::PropertiesGroup.t().to_owned(), name_of(m.gid(), false)),
        ],
    };
    let offset = crate::Linux::utc_offset_minutes();
    let changed = UNIX_EPOCH + std::time::Duration::new(m.ctime().max(0) as u64, m.ctime_nsec().clamp(0, 999_999_999) as u32);
    let dates = [(S::PropertiesCreated, m.created().ok()), (S::PropertiesAccessed, m.accessed().ok()), (S::PropertiesChanged, Some(changed))];
    let rows = dates.into_iter().filter_map(|(label, t)| Some((label.t().to_owned(), date(t?, offset)?))).collect();
    vec![access, PropGroup { title: S::PropertiesDates.t().to_owned(), rows }]
}

#[cfg(test)]
#[path = "../test/props.rs"]
mod tests;
