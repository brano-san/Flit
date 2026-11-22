use flit_core::update::ManagedInstallation;
use std::ffi::OsStr;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    Dpkg,
    Rpm,
}

pub fn appimage_path<'a>(exe: &Path, appimage: Option<&'a OsStr>, appdir: Option<&OsStr>) -> Option<&'a Path> {
    let image = Path::new(appimage?);
    let dir = Path::new(appdir?);
    let absolute = |path: &Path| path.as_os_str().as_encoded_bytes().starts_with(b"/") && !path.components().any(|part| part == std::path::Component::ParentDir);
    if !absolute(image) || !absolute(dir) || dir.parent().is_none() || image.starts_with(dir) {
        return None;
    }
    exe.strip_prefix(dir).ok().filter(|relative| !relative.as_os_str().is_empty() && !relative.components().any(|part| part == std::path::Component::ParentDir)).map(|_| image)
}

fn detect(exe: &Path, appimage: Option<&OsStr>, appdir: Option<&OsStr>, mut query: impl FnMut(Probe, &Path) -> Option<String>) -> Option<ManagedInstallation> {
    if appimage_path(exe, appimage, appdir).is_some() {
        return Some(ManagedInstallation::AppImage);
    }
    if let Some(package) = query(Probe::Dpkg, exe).as_deref().and_then(|text| dpkg_owner(text, exe)) {
        return Some(ManagedInstallation::Package(format!("sudo apt install --only-upgrade {package}")));
    }
    query(Probe::Rpm, exe).as_deref().and_then(|text| text.lines().find_map(package_name)).map(|package| ManagedInstallation::Package(format!("sudo dnf upgrade {package}")))
}

fn package_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (name.starts_with(|c: char| c.is_ascii_alphanumeric()) && name.bytes().all(|c| c.is_ascii_alphanumeric() || b".+_:-".contains(&c))).then_some(name)
}

fn dpkg_owner<'a>(text: &'a str, exe: &Path) -> Option<&'a str> {
    text.lines().filter_map(|line| line.split_once(": ")).find_map(|(owners, path)| (Path::new(path) == exe).then(|| owners.split(',').find_map(package_name)).flatten())
}

#[cfg(not(windows))]
pub fn managed(exe: &Path) -> Option<ManagedInstallation> {
    let appimage = std::env::var_os("APPIMAGE");
    let appdir = std::env::var_os("APPDIR");
    detect(exe, appimage.as_deref(), appdir.as_deref(), |probe, path| {
        let (program, args): (&str, &[&str]) = match probe {
            Probe::Dpkg => ("dpkg", &["-S", "--"]),
            Probe::Rpm => ("rpm", &["-qf", "--qf", "%{NAME}\n", "--"]),
        };
        let result = std::process::Command::new(program).args(args).arg(path).env("LC_ALL", "C").output().ok()?;
        result.status.success().then(|| String::from_utf8_lossy(&result.stdout).into_owned())
    })
}

#[cfg(test)]
#[path = "../test/install_format.rs"]
mod tests;
