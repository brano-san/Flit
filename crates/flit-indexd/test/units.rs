fn repo(path: &str) -> String {
    let full = format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{full}: {e}")).replace("\r\n", "\n")
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).collect()
}

const BIN: &str = env!("CARGO_BIN_NAME");

#[test]
fn the_user_unit_starts_this_binary_at_login_with_its_socket() {
    let unit = repo("assets/flit-indexd.service");
    let l = lines(&unit);
    assert!(l.contains(&format!("ExecStart=/usr/bin/{BIN}").as_str()), "{unit}");
    assert!(l.contains(&"WantedBy=default.target") && l.contains(&"Also=flit-indexd.socket"), "{unit}");
    assert!(!unit.contains("@BINDIR@"), "packages install the unit as is");
    let socket = repo("assets/flit-indexd.socket");
    assert!(lines(&socket).contains(&"WantedBy=sockets.target"), "{socket}");
}

#[test]
fn every_linux_package_ships_the_service_binary_and_both_units() {
    let tarball = repo("scripts/package-linux.sh");
    assert!(tarball.contains(&format!("-p {BIN} ")), "the archive build includes the service");
    assert!(tarball.contains(&format!("\"$target/dist/{BIN}\" \"$stage/bin/{BIN}\"")));
    assert!(tarball.contains("\"$root/assets/flit-indexd.service\" \"$root/assets/flit-indexd.socket\" \"$stage/share/flit/systemd/\""));
    let install = repo("scripts/install-linux.sh");
    assert!(install.contains("units=$prefix/share/systemd/user") && install.contains(&format!("\"$prefix/bin/{BIN}\"")));
    let manifest = repo("crates/flit/Cargo.toml");
    for unit in ["flit-indexd.service", "flit-indexd.socket"] {
        assert!(manifest.contains(&format!("[\"../../assets/{unit}\", \"usr/lib/systemd/user/\", \"644\"]")), "deb lacks {unit}");
        assert!(manifest.contains(&format!("dest = \"/usr/lib/systemd/user/{unit}\"")), "rpm lacks {unit}");
        assert!(repo("packaging/aur/PKGBUILD").contains(&format!("assets/{unit} \"$pkgdir/usr/lib/systemd/user/{unit}\"")), "AUR lacks {unit}");
    }
    assert!(manifest.contains(&format!("[\"target/release/{BIN}\", \"usr/bin/\", \"755\"]")));
    assert!(manifest.contains(&format!("dest = \"/usr/bin/{BIN}\"")));
}
