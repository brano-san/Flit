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
    Terminal,
    OnDemand,
}

struct Need {
    tools: &'static [&'static str],
    purpose: S,
    packages: [&'static str; 4],
    when: When,
}

const NEEDS: [Need; 22] = [
    Need { tools: &["xdg-open"], purpose: S::SettingsPageToolOpen, packages: ["xdg-utils", "xdg-utils", "xdg-utils", "xdg-utils"], when: When::Always },
    Need { tools: &["gdbus"], purpose: S::SettingsPageToolDesktop, packages: ["libglib2.0-bin", "glib2", "glib2", "glib2-tools"], when: When::Always },
    Need { tools: &["gio"], purpose: S::SettingsPageToolDrives, packages: ["libglib2.0-bin", "glib2", "glib2", "glib2-tools"], when: When::Always },
    Need { tools: &["gvfsd-network"], purpose: S::PathNetwork, packages: ["gvfs-backends", "gvfs", "gvfs", "gvfs"], when: When::Always },
    Need { tools: &["avahi-browse"], purpose: S::PathNetwork, packages: ["avahi-utils", "avahi-tools", "avahi", "avahi-utils"], when: When::Always },
    Need { tools: &["xclip"], purpose: S::SettingsPageToolClipboard, packages: ["xclip", "xclip", "xclip", "xclip"], when: When::X11 },
    Need { tools: &["wl-copy"], purpose: S::SettingsPageToolClipboard, packages: ["wl-clipboard", "wl-clipboard", "wl-clipboard", "wl-clipboard"], when: When::Wayland },
    Need { tools: &["wl-paste"], purpose: S::SettingsPageToolClipboard, packages: ["wl-clipboard", "wl-clipboard", "wl-clipboard", "wl-clipboard"], when: When::Wayland },
    Need { tools: &["udisksctl"], purpose: S::SettingsPageToolDrives, packages: ["udisks2", "udisks2", "udisks2", "udisks2"], when: When::Always },
    Need { tools: &["pkexec"], purpose: S::SettingsPageToolAdmin, packages: ["pkexec", "polkit", "polkit", "polkit"], when: When::Always },
    Need { tools: &["setfacl"], purpose: S::SettingsPageToolAdmin, packages: ["acl", "acl", "acl", "acl"], when: When::Always },
    Need { tools: &["fusermount3", "fusermount"], purpose: S::SettingsPageToolDrives, packages: ["fuse3", "fuse3", "fuse3", "fuse3"], when: When::Always },
    Need { tools: &["umount"], purpose: S::SettingsPageToolDrives, packages: ["mount", "util-linux-core", "util-linux", "util-linux"], when: When::Always },
    Need { tools: &["curl"], purpose: S::SettingsPageToolUpdate, packages: ["curl", "curl", "curl", "curl"], when: When::Always },
    Need { tools: &["recollq", "recoll", "localsearch", "tracker3", "baloosearch6", "baloosearch"], purpose: S::SettingsPageToolSearch, packages: ["recollcmd", "recoll", "recoll", "recoll"], when: When::Always },
    Need { tools: &["pdftoppm"], purpose: S::SettingsPageToolPdf, packages: ["poppler-utils", "poppler-utils", "poppler", "poppler-tools"], when: When::Always },
    Need { tools: &["ffmpegthumbnailer"], purpose: S::SettingsPageToolVideo, packages: ["ffmpegthumbnailer", "ffmpegthumbnailer", "ffmpegthumbnailer", "ffmpegthumbnailer"], when: When::Always },
    Need { tools: &["mpv", "gst-play-1.0", "ffplay", "paplay"], purpose: S::SettingsPageToolMedia, packages: ["mpv", "mpv", "mpv", "mpv"], when: When::Always },
    Need { tools: &["libmpv"], purpose: S::SettingsPageToolMediaInside, packages: ["libmpv2", "mpv-libs", "mpv", "libmpv2"], when: When::Always },
    Need { tools: &["lp"], purpose: S::SettingsPageToolPrint, packages: ["cups-client", "cups-client", "cups", "cups-client"], when: When::Always },
    Need { tools: &["xterm"], purpose: S::SettingsPageToolTerminal, packages: ["xterm", "xterm", "xterm", "xterm"], when: When::Terminal },
    Need { tools: &["gh"], purpose: S::GitErrorGithubLogin, packages: ["gh", "gh", "github-cli", "gh"], when: When::OnDemand },
];

const LIBRARIES: [(&str, &[&str]); 1] = [("libmpv", &crate::mpv::LIBRARIES)];
const OLD_DEBIAN: [(&str, &str); 1] = [("libmpv", "libmpv1")];
const OLD_DEBIAN_RELEASES: [&str; 6] = ["bionic", "focal", "jammy", "kinetic", "buster", "bullseye"];
const LIBRARY_DIRS: [&str; 5] = ["/usr/lib", "/usr/lib64", "/lib", "/lib64", "/usr/local/lib"];

fn value(os_release: &str, key: &str) -> String {
    os_release.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim_matches('"').to_ascii_lowercase()).unwrap_or_default()
}

pub fn family(os_release: &str) -> usize {
    let value = |key: &str| value(os_release, key);
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
    NEEDS.iter().find(|n| n.tools.first() == Some(&tool)).map(|n| package(n, os_release))
}

fn package(need: &Need, os_release: &str) -> &'static str {
    let f = family(os_release);
    let old = f == DEBIAN && ["UBUNTU_CODENAME=", "VERSION_CODENAME="].iter().any(|k| OLD_DEBIAN_RELEASES.contains(&value(os_release, k).as_str()));
    OLD_DEBIAN.iter().find(|(tool, _)| old && need.tools[0] == *tool).map_or(need.packages[f], |(_, p)| p)
}

pub fn library_listed(cache: &[u8], file: &str) -> bool {
    let entry = [b"/", file.as_bytes(), b"\0"].concat();
    cache.windows(entry.len()).any(|w| w == entry)
}

fn library_present(files: &[&str], cache: &[u8], dirs: &[std::path::PathBuf]) -> bool {
    files.iter().any(|f| library_listed(cache, f) || dirs.iter().any(|d| d.join(f).is_file()))
}

fn library_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("LD_LIBRARY_PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let multiarch = format!("{}-linux-gnu", std::env::consts::ARCH);
    dirs.extend(LIBRARY_DIRS.iter().map(std::path::PathBuf::from));
    dirs.extend(["/usr/lib", "/lib"].iter().map(|d| std::path::Path::new(d).join(&multiarch)));
    dirs
}

fn found(tool: &str, cache: &dyn Fn() -> Vec<u8>) -> bool {
    if tool == "gvfsd-network" {
        return ["/usr/libexec", "/usr/lib", "/usr/lib/gvfs", "/usr/lib64/gvfs", "/usr/lib/gvfs-daemons"].iter().any(|dir| std::path::Path::new(dir).join(tool).is_file());
    }
    match LIBRARIES.iter().find(|(name, _)| *name == tool) {
        Some((_, files)) => crate::mpv::loaded() || library_present(files, &cache(), &library_dirs()),
        None => crate::shell::on_path(OsStr::new(tool)).is_some(),
    }
}

pub(crate) fn require(tool: &str) -> Result<std::path::PathBuf, flit_platform::PlatformError> {
    crate::shell::on_path(OsStr::new(tool)).ok_or_else(|| crate::shell::tool_missing(tool))
}

pub(crate) fn command(tool: &str) -> std::io::Result<std::process::Command> {
    require(tool).map(std::process::Command::new).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.message))
}

pub fn missing_from(found: impl Fn(&str) -> bool, os_release: &str, wayland: bool, terminal: bool) -> Vec<MissingTool> {
    let applies = |w: When| match w {
        When::Always => true,
        When::X11 => !wayland,
        When::Wayland => wayland,
        When::Terminal => !terminal,
        When::OnDemand => false,
    };
    NEEDS
        .iter()
        .filter(|n| applies(n.when) && !n.tools.iter().any(|t| found(t)))
        .map(|n| MissingTool { tool: n.tools[0], purpose: n.purpose, package: package(n, os_release) })
        .collect()
}

pub fn os_release() -> String {
    std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default()
}

pub fn missing() -> Vec<MissingTool> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let cache = || std::fs::read("/etc/ld.so.cache").unwrap_or_default();
    missing_from(|t| found(t, &cache), &os_release(), wayland, crate::shell::terminal_program().is_some())
}

#[cfg(test)]
#[path = "../test/tools.rs"]
mod tests;
