use super::*;

const UBUNTU: &str = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
const FEDORA_RELEASE: &str = "NAME=\"Fedora Linux\"\nID=fedora\n";
const MANJARO: &str = "ID=manjaro\nID_LIKE=arch\n";
const TUMBLEWEED: &str = "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";

#[test]
fn missing_helpers_are_listed_with_the_package_of_this_distribution() {
    let have = ["gdbus", "gio", "udisksctl", "pkexec", "setfacl", "fusermount", "umount", "curl", "recollq", "pdftoppm", "ffmpegthumbnailer", "lp", "paplay", "xclip"];
    let found = |t: &str| have.contains(&t);
    let missing = missing_from(found, FEDORA_RELEASE, false, true);
    assert_eq!(missing.iter().map(|m| (m.tool, m.package)).collect::<Vec<_>>(), [("xdg-open", "xdg-utils")], "paplay plays audio when mpv is missing");
    let wayland = missing_from(found, UBUNTU, true, false);
    assert_eq!(wayland.iter().map(|m| m.package).collect::<Vec<_>>(), ["xdg-utils", "wl-clipboard", "wl-clipboard", "xterm"]);
    assert_eq!(missing_from(|_| false, MANJARO, false, true).iter().find(|m| m.tool == "pdftoppm").map(|m| m.package), Some("poppler"));
    assert_eq!(package_for("gdbus", TUMBLEWEED), Some("glib2-tools"));
    assert_eq!(package_for("gdbus", ""), Some("libglib2.0-bin"), "unknown systems get Debian names");
}

#[test]
fn nothing_is_listed_when_every_helper_is_present() {
    assert!(missing_from(|_| true, UBUNTU, false, true).is_empty());
    assert!(missing_from(|_| true, UBUNTU, true, true).is_empty());
    assert_eq!(package_for("ls", UBUNTU), None);
    assert_eq!(package_for("baloosearch", UBUNTU), None);
    assert_eq!(package_for("ffplay", UBUNTU), None);
}

#[test]
fn a_missing_xdg_open_names_the_package_to_install() {
    assert_eq!(crate::shell::no_xdg_open(), S::ErrorsOpsExtraNoXdgOpen.en().replace("{package}", "xdg-utils"));
}

#[test]
fn missing_helper_packages_match_all_four_distribution_families() {
    for (tool, packages) in [
        ("gio", ["libglib2.0-bin", "glib2", "glib2", "glib2-tools"]),
        ("setfacl", ["acl"; 4]),
        ("curl", ["curl"; 4]),
        ("wl-paste", ["wl-clipboard"; 4]),
        ("fusermount", ["fuse", "fuse", "fuse2", "fuse"]),
        ("umount", ["mount", "util-linux-core", "util-linux", "util-linux"]),
        ("xterm", ["xterm"; 4]),
        ("recollq", ["recollcmd", "recoll", "recoll", "recoll"]),
    ] {
        for (release, package) in [UBUNTU, FEDORA_RELEASE, MANJARO, TUMBLEWEED].into_iter().zip(packages) {
            assert_eq!(package_for(tool, release), Some(package), "{tool}: {release}");
        }
    }
}

#[test]
fn missing_independent_helpers_are_not_hidden_by_another_program_from_the_same_package() {
    let missing = missing_from(|t| t != "gio" && t != "wl-paste", UBUNTU, true, true);
    assert_eq!(missing.iter().map(|m| (m.tool, m.package)).collect::<Vec<_>>(), [("gio", "libglib2.0-bin"), ("wl-paste", "wl-clipboard")]);
}
