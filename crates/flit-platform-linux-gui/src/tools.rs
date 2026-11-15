use flit_core::i18n::S;
use flit_core::ui::MissingTool;
use std::ffi::OsStr;

const DEBIAN: usize = 0;
const FEDORA: usize = 1;
const ARCH: usize = 2;
const SUSE: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum When {
    Always,
    X11,
    Wayland,
}

struct Need {
    tools: &'static [&'static str],
    purpose: S,
    packages: [&'static str; 4],
    when: When,
}

const NEEDS: [Need; 17] = [
    Need { tools: &["xdg-open"], purpose: S::SettingsPageToolOpen, packages: ["xdg-utils", "xdg-utils", "xdg-utils", "xdg-utils"], when: When::Always },
    Need { tools: &["gdbus"], purpose: S::SettingsPageToolDesktop, packages: ["libglib2.0-bin", "glib2", "glib2", "glib2-tools"], when: When::Always },
    Need { tools: &["gio"], purpose: S::SettingsPageToolDrives, packages: ["libglib2.0-bin", "glib2", "glib2", "glib2-tools"], when: When::Always },
    Need { tools: &["xclip"], purpose: S::SettingsPageToolClipboard, packages: ["xclip", "xclip", "xclip", "xclip"], when: When::X11 },
    Need { tools: &["wl-copy"], purpose: S::SettingsPageToolClipboard, packages: ["wl-clipboard", "wl-clipboard", "wl-clipboard", "wl-clipboard"], when: When::Wayland },
    Need { tools: &["wl-paste"], purpose: S::SettingsPageToolClipboard, packages: ["wl-clipboard", "wl-clipboard", "wl-clipboard", "wl-clipboard"], when: When::Wayland },
    Need { tools: &["udisksctl"], purpose: S::SettingsPageToolDrives, packages: ["udisks2", "udisks2", "udisks2", "udisks2"], when: When::Always },
    Need { tools: &["pkexec"], purpose: S::SettingsPageToolAdmin, packages: ["pkexec", "polkit", "polkit", "polkit"], when: When::Always },
    Need { tools: &["setfacl"], purpose: S::SettingsPageToolAdmin, packages: ["acl", "acl", "acl", "acl"], when: When::Always },
    Need { tools: &["fusermount"], purpose: S::SettingsPageToolDrives, packages: ["fuse", "fuse", "fuse2", "fuse"], when: When::Always },
    Need { tools: &["umount"], purpose: S::SettingsPageToolDrives, packages: ["mount", "util-linux-core", "util-linux", "util-linux"], when: When::Always },
    Need { tools: &["curl"], purpose: S::SettingsPageToolUpdate, packages: ["curl", "curl", "curl", "curl"], when: When::Always },
    Need { tools: &["recollq", "recoll", "localsearch", "tracker3", "baloosearch6", "baloosearch"], purpose: S::SettingsPageToolSearch, packages: ["recollcmd", "recoll", "recoll", "recoll"], when: When::Always },
    Need { tools: &["pdftoppm"], purpose: S::SettingsPageToolPdf, packages: ["poppler-utils", "poppler-utils", "poppler", "poppler-tools"], when: When::Always },
    Need { tools: &["ffmpegthumbnailer"], purpose: S::SettingsPageToolVideo, packages: ["ffmpegthumbnailer", "ffmpegthumbnailer", "ffmpegthumbnailer", "ffmpegthumbnailer"], when: When::Always },
    Need { tools: &["mpv", "gst-play-1.0", "ffplay", "paplay"], purpose: S::SettingsPageToolMedia, packages: ["mpv", "mpv", "mpv", "mpv"], when: When::Always },
    Need { tools: &["lp"], purpose: S::SettingsPageToolPrint, packages: ["cups-client", "cups-client", "cups", "cups-client"], when: When::Always },
];

pub fn family(os_release: &str) -> usize {
    let value = |key: &str| os_release.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim_matches('"').to_ascii_lowercase()).unwrap_or_default();
    let names = format!("{} {}", value("ID="), value("ID_LIKE="));
    let has = |n: &str| names.split_whitespace().any(|w| w == n);
    if has("fedora") || has("rhel") || has("centos") {
        FEDORA
    } else if has("arch") {
        ARCH
    } else if has("suse") || has("opensuse") || names.contains("opensuse") {
        SUSE
    } else {
        DEBIAN
    }
}

pub fn package_for(tool: &str, os_release: &str) -> Option<&'static str> {
    if tool == "xterm" {
        return Some("xterm");
    }
    NEEDS.iter().find(|n| n.tools.first() == Some(&tool)).map(|n| n.packages[family(os_release)])
}

pub(crate) fn require(tool: &str) -> Result<std::path::PathBuf, flit_platform::PlatformError> {
    crate::shell::on_path(OsStr::new(tool)).ok_or_else(|| crate::shell::tool_missing(tool))
}

pub(crate) fn command(tool: &str) -> std::io::Result<std::process::Command> {
    require(tool).map(std::process::Command::new).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.message))
}

pub fn missing_from(found: impl Fn(&str) -> bool, os_release: &str, wayland: bool, terminal: bool) -> Vec<MissingTool> {
    let f = family(os_release);
    let applies = |w: When| w == When::Always || (w == When::Wayland) == wayland;
    let mut out: Vec<MissingTool> = NEEDS
        .iter()
        .filter(|n| applies(n.when) && !n.tools.iter().any(|t| found(t)))
        .map(|n| MissingTool { tool: n.tools[0], purpose: n.purpose, package: n.packages[f] })
        .collect();
    if !terminal {
        out.push(MissingTool { tool: "xterm", purpose: S::SettingsPageToolTerminal, package: "xterm" });
    }
    out
}

pub fn os_release() -> String {
    std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default()
}

pub fn missing() -> Vec<MissingTool> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    missing_from(|t| crate::shell::on_path(OsStr::new(t)).is_some(), &os_release(), wayland, crate::shell::terminal_program().is_some())
}

#[cfg(test)]
#[path = "../test/tools.rs"]
mod tests;
