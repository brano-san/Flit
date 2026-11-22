use super::*;

const UBUNTU: &str = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
const FEDORA_RELEASE: &str = "NAME=\"Fedora Linux\"\nID=fedora\n";
const MANJARO: &str = "ID=manjaro\nID_LIKE=arch\n";
const TUMBLEWEED: &str = "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";

#[test]
fn missing_helpers_are_listed_with_the_package_of_this_distribution() {
    let have = ["gdbus", "gio", "udisksctl", "pkexec", "setfacl", "fusermount", "umount", "curl", "recollq", "pdftoppm", "pdfinfo", "ffmpegthumbnailer", "lp", "paplay", "xclip", "libmpv"];
    let found = |t: &str| have.contains(&t);
    let missing = missing_from(found, FEDORA_RELEASE, false, true);
    assert_eq!(missing.iter().map(|m| (m.tool, m.package)).collect::<Vec<_>>(), [("xdg-open", "xdg-utils"), ("gvfsd-network", "gvfs"), ("avahi-browse", "avahi-tools")], "paplay plays audio when mpv is missing");
    let wayland = missing_from(found, UBUNTU, true, false);
    assert_eq!(wayland.iter().map(|m| m.package).collect::<Vec<_>>(), ["xdg-utils", "gvfs-backends", "avahi-utils", "wl-clipboard", "wl-clipboard", "xterm"]);
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
        ("fusermount3", ["fuse3"; 4]),
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

#[test]
fn pdfinfo_and_pdftoppm_are_independently_reported_as_missing() {
    for tool in ["pdfinfo", "pdftoppm"] {
        let missing = missing_from(|t| t != tool, UBUNTU, false, true);
        assert_eq!(missing.iter().map(|m| (m.tool, m.package)).collect::<Vec<_>>(), [(tool, "poppler-utils")]);
    }
}

#[test]
fn fuse3_only_and_fuse2_only_systems_both_have_an_unmount_helper() {
    let only = |tool: &'static str| move |t: &str| t != "fusermount3" && t != "fusermount" || t == tool;
    for tool in ["fusermount3", "fusermount"] {
        assert!(!missing_from(only(tool), UBUNTU, false, true).iter().any(|m| m.purpose == S::SettingsPageToolDrives), "{tool}");
    }
    let none = missing_from(|t| t != "fusermount3" && t != "fusermount", UBUNTU, false, true);
    assert_eq!(none.iter().map(|m| (m.tool, m.package)).collect::<Vec<_>>(), [("fusermount3", "fuse3")]);
}

#[test]
fn the_terminal_entry_comes_from_the_table_and_only_when_no_terminal_exists() {
    assert!(missing_from(|t| t != "xterm", UBUNTU, false, true).is_empty());
    let missing = missing_from(|t| t != "xterm", TUMBLEWEED, false, false);
    assert_eq!(missing.iter().map(|m| (m.tool, m.purpose, m.package)).collect::<Vec<_>>(), [("xterm", S::SettingsPageToolTerminal, "xterm")]);
}

const JAMMY: &str = "NAME=\"Ubuntu\"\nVERSION_ID=\"22.04\"\nID=ubuntu\nID_LIKE=debian\nVERSION_CODENAME=jammy\nUBUNTU_CODENAME=jammy\n";
const NOBLE: &str = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\nVERSION_CODENAME=noble\nUBUNTU_CODENAME=noble\n";
const MINT: &str = "NAME=\"Linux Mint\"\nID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_CODENAME=virginia\nUBUNTU_CODENAME=jammy\n";
const BULLSEYE: &str = "ID=debian\nVERSION_CODENAME=bullseye\n";
const BOOKWORM: &str = "ID=debian\nVERSION_CODENAME=bookworm\n";

#[test]
fn a_missing_libmpv_names_the_library_package_of_each_distribution_and_release() {
    for (release, package) in [(JAMMY, "libmpv1"), (MINT, "libmpv1"), (BULLSEYE, "libmpv1"), (NOBLE, "libmpv2"), (BOOKWORM, "libmpv2"), (UBUNTU, "libmpv2"), (FEDORA_RELEASE, "mpv-libs"), (MANJARO, "mpv"), (TUMBLEWEED, "libmpv2")] {
        assert_eq!(package_for("libmpv", release), Some(package), "{release}");
    }
    let missing = missing_from(|t| t != "libmpv", JAMMY, false, true);
    assert_eq!(missing.iter().map(|m| (m.tool, m.purpose, m.package)).collect::<Vec<_>>(), [("libmpv", S::SettingsPageToolMediaInside, "libmpv1")]);
}

#[test]
fn the_old_release_rule_touches_only_libmpv_and_mpv_alone_still_plays() {
    assert_eq!(package_for("mpv", JAMMY), Some("mpv"));
    assert_eq!(package_for("pdftoppm", BULLSEYE), Some("poppler-utils"));
    assert_eq!(package_for("libmpv", "ID=fedora\nVERSION_CODENAME=jammy\n"), Some("mpv-libs"), "a codename alone does not make a system Debian");
    let missing = missing_from(|t| t != "mpv" && t != "gst-play-1.0" && t != "ffplay" && t != "paplay", NOBLE, false, true);
    assert_eq!(missing.iter().map(|m| m.tool).collect::<Vec<_>>(), ["mpv"], "with libmpv the separate player is still listed on its own");
}

#[test]
fn libmpv_is_found_in_the_loader_cache_or_a_library_folder() {
    let cache = b"glibc-ld.so.cache1.1\0libmpv.so.1\0/lib/x86_64-linux-gnu/libmpv.so.1\0libz.so.1\0/lib/x86_64-linux-gnu/libz.so.1\0";
    assert!(library_listed(cache, "libmpv.so.1"));
    assert!(!library_listed(cache, "libmpv.so.2"));
    assert!(!library_listed(cache, "mpv.so.1"), "a name inside another name does not count");
    assert!(!library_listed(b"/usr/lib/libmpv.so.10\0", "libmpv.so.1"));
    assert!(!library_listed(b"", "libmpv.so.1"));
    let dir = crate::temp::Temp::new("libs");
    let dirs = [dir.path().to_path_buf()];
    assert!(!library_present(&crate::mpv::LIBRARIES, b"", &dirs));
    dir.file("libmpv.so.2", 4);
    assert!(library_present(&crate::mpv::LIBRARIES, b"", &dirs));
    assert!(library_present(&crate::mpv::LIBRARIES, cache, &[]));
}

#[test]
fn only_missing_network_tools_are_listed_with_their_packages() {
    for (release, gvfs, avahi) in [(UBUNTU, "gvfs-backends", "avahi-utils"), (FEDORA_RELEASE, "gvfs", "avahi-tools"), (MANJARO, "gvfs", "avahi"), (TUMBLEWEED, "gvfs", "avahi-utils")] {
        let missing = missing_from(|t| !matches!(t, "gvfsd-network" | "avahi-browse"), release, false, true);
        assert_eq!(missing.iter().map(|m| (m.tool, m.purpose, m.package)).collect::<Vec<_>>(), [
            ("gvfsd-network", S::PathNetwork, gvfs),
            ("avahi-browse", S::PathNetwork, avahi),
        ]);
        assert!(missing_from(|_| true, release, false, true).is_empty());
    }
}
