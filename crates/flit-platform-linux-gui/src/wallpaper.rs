use flit_core::i18n::S;
use flit_core::tr_en;
use flit_platform::PlatformError;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

type Attempt = Vec<Vec<String>>;

fn gsettings(schema: &str, key: &str, value: &str) -> Vec<String> {
    ["gsettings", "set", schema, key, value].map(str::to_owned).to_vec()
}

pub(crate) fn attempts(path: &Path, desktop: &str) -> Vec<Attempt> {
    let shown = path.to_string_lossy().into_owned();
    let uri = format!("file://{}", flit_platform_linux::percent_encode(path.as_os_str().as_bytes()));
    let portal = vec![[
        "gdbus",
        "call",
        "--session",
        "--dest",
        "org.freedesktop.portal.Desktop",
        "--object-path",
        "/org/freedesktop/portal/desktop",
        "--method",
        "org.freedesktop.portal.Wallpaper.SetWallpaperURI",
        "",
        uri.as_str(),
        "{'show-preview': <false>, 'set-on': <'background'>}",
    ]
    .map(str::to_owned)
    .to_vec()];
    let desktop = desktop.to_ascii_lowercase();
    let is = |names: &[&str]| desktop.split(':').any(|d| names.contains(&d));
    let mut out = Vec::new();
    if is(&["kde"]) {
        out.push(vec![vec!["plasma-apply-wallpaperimage".to_owned(), shown.clone()]]);
    } else if is(&["x-cinnamon", "cinnamon"]) {
        out.push(vec![gsettings("org.cinnamon.desktop.background", "picture-uri", &uri)]);
    } else if is(&["mate"]) {
        out.push(vec![gsettings("org.mate.background", "picture-filename", &shown)]);
    } else if is(&["gnome", "unity", "ubuntu", "budgie", "pantheon"]) {
        out.push(vec![gsettings("org.gnome.desktop.background", "picture-uri", &uri), gsettings("org.gnome.desktop.background", "picture-uri-dark", &uri)]);
    }
    out.push(portal);
    out
}

fn succeeded(cmd: &[String]) -> bool {
    Command::new(&cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn set(path: &Path) -> Result<(), PlatformError> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    for attempt in attempts(path, &desktop) {
        let Some((first, rest)) = attempt.split_first() else { continue };
        if succeeded(first) {
            for cmd in rest {
                succeeded(cmd);
            }
            return Ok(());
        }
    }
    Err(PlatformError::new(-1, tr_en!(S::ErrorsOpsExtraWallpaperUnsupported)))
}

#[cfg(test)]
#[path = "../test/wallpaper.rs"]
mod tests;
