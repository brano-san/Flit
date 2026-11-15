use flit_core::input::{WindowButton, WindowButtons};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Desktop {
    Gnome,
    Kde,
    Other,
}

impl Desktop {
    pub(crate) fn from_name(name: &str) -> Self {
        let has = |part: &str| name.split(':').any(|s| s.eq_ignore_ascii_case(part));
        if has("KDE") { Self::Kde } else if has("GNOME") { Self::Gnome } else { Self::Other }
    }

    pub(crate) fn current() -> Self {
        Self::from_name(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
    }
}

fn buttons(left: impl Iterator<Item = WindowButton>, right: impl Iterator<Item = WindowButton>) -> WindowButtons {
    let mut seen = [false; 3];
    let left = left.filter(|b| !std::mem::replace(&mut seen[b.index()], true)).collect();
    let right = right.filter(|b| !std::mem::replace(&mut seen[b.index()], true)).collect();
    WindowButtons { left, right }
}

pub(crate) fn gnome_buttons(layout: &str) -> WindowButtons {
    let (left, right) = layout.split_once(':').unwrap_or((layout, ""));
    let parse = |part: &str| part.split(',').filter_map(|word| match word.trim() {
        "minimize" => Some(WindowButton::Minimize),
        "maximize" => Some(WindowButton::Maximize),
        "close" => Some(WindowButton::Close),
        _ => None,
    }).collect::<Vec<_>>();
    buttons(parse(left).into_iter(), parse(right).into_iter())
}

pub(crate) fn kde_buttons(config: &str) -> WindowButtons {
    let (mut left, mut right) = ("MS", "HIAX");
    let mut section = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line == "[org.kde.kdecoration2]";
        } else if section {
            if let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "ButtonsOnLeft" => left = value.trim(),
                    "ButtonsOnRight" => right = value.trim(),
                    _ => {}
                }
            }
        }
    }
    let parse = |part: &str| part.chars().filter_map(|c| match c {
        'I' => Some(WindowButton::Minimize),
        'A' => Some(WindowButton::Maximize),
        'X' => Some(WindowButton::Close),
        _ => None,
    }).collect::<Vec<_>>();
    buttons(parse(left).into_iter(), parse(right).into_iter())
}

pub(crate) const KDE_ICONS: &str = "breeze";

pub(crate) fn kde_icons_or_default(config: &str) -> String {
    kde_icon_theme(config).unwrap_or_else(|| KDE_ICONS.into())
}

pub(crate) fn kde_icon_theme(config: &str) -> Option<String> {
    let mut icons = false;
    let mut theme = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            icons = line == "[Icons]";
        } else if icons {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "Theme" {
                    theme = Some(value.trim().trim_matches(['\'', '"']).to_owned()).filter(|v| !v.is_empty());
                }
            }
        }
    }
    theme
}

fn read_config(path: &Path) -> String {
    let mut text = String::new();
    let _ = std::fs::File::open(path).and_then(|f| f.take(1 << 20).read_to_string(&mut text));
    text
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Buttons(WindowButtons),
    Globals(String),
    IconTheme(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp(u64, u64, u64, i64, i64, i64, i64);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp(m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()))
}

pub(crate) struct Watch {
    stop: Option<crossbeam_channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    pub(crate) fn start(config: PathBuf, period: Duration, emit: impl Fn(Change) + Send + 'static) -> std::io::Result<Self> {
        let (stop, rx) = crossbeam_channel::bounded(1);
        let thread = std::thread::Builder::new().name("flit-desktop-config".into()).spawn(move || {
            let paths = [config.join("kwinrc"), config.join("kdeglobals")];
            let mut stamps = [None, None];
            let mut layout = None;
            let mut globals = String::new();
            let mut icon_theme = Some(KDE_ICONS.to_owned());
            let mut first = true;
            loop {
                for (k, path) in paths.iter().enumerate() {
                    let now = stamp(path);
                    if !first && now == stamps[k] { continue; }
                    stamps[k] = now;
                    let text = read_config(path);
                    if k == 0 {
                        let next = kde_buttons(&text);
                        if layout.as_ref() != Some(&next) {
                            layout = Some(next.clone());
                            emit(Change::Buttons(next));
                        }
                    } else if globals != text {
                        let next = Some(kde_icons_or_default(&text));
                        if next != icon_theme {
                            icon_theme.clone_from(&next);
                            if let Some(name) = next { emit(Change::IconTheme(name)); }
                        }
                        globals.clone_from(&text);
                        emit(Change::Globals(text));
                    }
                }
                first = false;
                match rx.recv_timeout(period) {
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    _ => return,
                }
            }
        })?;
        Ok(Self { stop: Some(stop), thread: Some(thread) })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "../test/preferences.rs"]
mod tests;
