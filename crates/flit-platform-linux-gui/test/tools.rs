use super::*;

const UBUNTU: &str = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
const FEDORA_RELEASE: &str = "NAME=\"Fedora Linux\"\nID=fedora\n";
const MANJARO: &str = "ID=manjaro\nID_LIKE=arch\n";
const TUMBLEWEED: &str = "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";

#[test]
fn missing_helpers_are_listed_with_the_package_of_this_distribution() {
    let have = ["gdbus", "udisksctl", "pkexec", "pdftoppm", "ffmpegthumbnailer", "lp", "paplay", "xclip"];
    let found = |t: &str| have.contains(&t);
    let missing = missing_from(found, FEDORA_RELEASE, false, true);
    assert_eq!(missing.iter().map(|m| (m.tool, m.package)).collect::<Vec<_>>(), [("xdg-open", "xdg-utils")], "paplay plays audio when mpv is missing");
    let wayland = missing_from(found, UBUNTU, true, false);
    assert_eq!(wayland.iter().map(|m| m.package).collect::<Vec<_>>(), ["xdg-utils", "wl-clipboard", "xterm"]);
    assert_eq!(missing_from(|_| false, MANJARO, false, true).iter().find(|m| m.tool == "pdftoppm").map(|m| m.package), Some("poppler"));
    assert_eq!(package_for("gdbus", TUMBLEWEED), Some("glib2-tools"));
    assert_eq!(package_for("gdbus", ""), Some("libglib2.0-bin"), "unknown systems get Debian names");
}

#[test]
fn nothing_is_listed_when_every_helper_is_present() {
    assert!(missing_from(|_| true, UBUNTU, false, true).is_empty());
    assert!(missing_from(|_| true, UBUNTU, true, true).is_empty());
    assert_eq!(package_for("ls", UBUNTU), None);
}

#[test]
fn a_missing_xdg_open_names_the_package_to_install() {
    assert_eq!(crate::shell::no_xdg_open(), S::ErrorsOpsExtraNoXdgOpen.en().replace("{package}", "xdg-utils"));
}
