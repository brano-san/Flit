use super::*;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("flit-indexd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_replaced_binary_is_noticed_and_an_untouched_one_is_not() {
    let d = Dir::new("binary");
    let exe = d.0.join("flit-indexd");
    std::fs::write(&exe, b"old").unwrap();
    let start = stamp(&exe).unwrap();
    assert_eq!(check(&exe, start), Binary::Same);
    std::fs::write(&exe, b"touched in place").unwrap();
    assert_eq!(check(&exe, start), Binary::Same, "only a new file is a new version");
    let fresh = d.0.join("flit-indexd.new");
    std::fs::write(&fresh, b"new").unwrap();
    std::fs::rename(&fresh, &exe).unwrap();
    assert_eq!(check(&exe, start), Binary::Replaced);
    std::fs::remove_file(&exe).unwrap();
    assert_eq!(check(&exe, start), Binary::Gone);
}

#[test]
fn old_snapshots_are_removed_but_other_files_stay() {
    let d = Dir::new("old");
    let old = d.0.join("indexd");
    std::fs::create_dir(&old).unwrap();
    for name in ["0123456789abcdef.idx", "fedcba9876543210.tmp", "notes.txt", "0123456789abcdef.bak"] {
        std::fs::write(old.join(name), b"x").unwrap();
    }
    assert_eq!(remove_old_snapshots(&old), 2);
    let mut left: Vec<String> = std::fs::read_dir(&old).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert_eq!(left, ["0123456789abcdef.bak", "notes.txt"]);
    std::fs::remove_file(old.join("notes.txt")).unwrap();
    std::fs::remove_file(old.join("0123456789abcdef.bak")).unwrap();
    std::fs::write(old.join("0123456789abcdef.idx"), b"x").unwrap();
    assert_eq!(remove_old_snapshots(&old), 1);
    assert!(!old.exists(), "a folder left empty goes too");
    assert_eq!(remove_old_snapshots(&old), 0);
}
