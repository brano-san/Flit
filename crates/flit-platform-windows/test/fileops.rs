use super::*;
use flit_platform::Shell;
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flit-ops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }
    fn file(&self, rel: &str, body: &str) -> FsPath {
        std::fs::write(self.0.join(rel), body).unwrap();
        self.path(rel)
    }
    fn has(&self, rel: &str) -> bool {
        self.0.join(rel).exists()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn info_file(recycled: &FsPath) -> String {
    let name = String::from_utf8_lossy(recycled.file_name()).replacen("$R", "$I", 1);
    recycled.parent().unwrap().join(name.as_bytes()).display().into_owned()
}

fn names(items: &[(FsPath, FsPath)]) -> Vec<String> {
    items.iter().map(|(_, to)| String::from_utf8_lossy(to.file_name()).into_owned()).collect()
}

#[test]
fn copy_move_rename_new_folder_and_link_report_their_results() {
    let t = TempDir::new("basic");
    let mut sh = ShellCom::new().unwrap();
    let a = t.file("a.txt", "alpha");
    std::fs::create_dir(t.0.join("sub")).unwrap();

    let out = sh.file_op(&FileOp::Copy { items: vec![a.clone()], dest: t.path("sub") });
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, vec![(a.clone(), t.path(r"sub/a.txt"))]);
    assert_eq!(std::fs::read_to_string(t.0.join(r"sub/a.txt")).unwrap(), "alpha");

    let out = sh.file_op(&FileOp::Copy { items: vec![a.clone()], dest: t.path("") });
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 1);
    assert_ne!(out.items[0].1, a, "a copy into the same folder gets a new name");
    assert!(Path::new(&*out.items[0].1.display()).exists());

    let out = sh.file_op(&FileOp::Rename { item: a.clone(), name: b"b.txt".to_vec() });
    assert_eq!((out.error.is_none(), out.items), (true, vec![(a.clone(), t.path("b.txt"))]));
    assert!(t.has("b.txt") && !t.has("a.txt"));

    let out = sh.file_op(&FileOp::NewFolder { dir: t.path(""), name: b"New folder".to_vec() });
    assert_eq!(names(&out.items), ["New folder"]);
    let out = sh.file_op(&FileOp::NewFolder { dir: t.path(""), name: b"New folder".to_vec() });
    assert_eq!(names(&out.items), ["New folder (2)"], "a taken name is renamed instead of failing");

    let out = sh.file_op(&FileOp::Move { items: vec![t.path("b.txt")], dest: t.path("New folder") });
    assert_eq!(out.items, vec![(t.path("b.txt"), t.path(r"New folder\b.txt"))]);
    let out = sh.file_op(&FileOp::MoveTo(vec![(t.path(r"New folder\b.txt"), t.path("a.txt"))]));
    assert_eq!(out.items, vec![(t.path(r"New folder\b.txt"), t.path("a.txt"))]);
    assert!(t.has("a.txt") && !t.has(r"New folder\b.txt"));

    let out = sh.file_op(&FileOp::Link { items: vec![t.path("a.txt")], dest: t.path("sub") });
    assert_eq!(names(&out.items), ["a.txt - Shortcut.lnk"]);
    let out = sh.file_op(&FileOp::Link { items: vec![t.path("a.txt")], dest: t.path("sub") });
    assert_eq!(names(&out.items), ["a.txt - Shortcut (2).lnk"]);
}

#[test]
fn unaddressable_names_are_refused_before_the_shell_sees_them() {
    let t = TempDir::new("refuse");
    let mut sh = ShellCom::new().unwrap();
    let a = t.file("a.txt", "x");
    let out = sh.file_op(&FileOp::Rename { item: a.clone(), name: b"b.".to_vec() });
    assert!(out.error.is_some() && out.items.is_empty());
    let out = sh.file_op(&FileOp::Copy { items: vec![t.path("trailing.")], dest: t.path("") });
    assert!(out.error.unwrap().message.contains("dot or a space"));
    assert!(t.has("a.txt"));
}

#[test]
fn trash_reports_the_recycled_item_and_restore_brings_it_back() {
    let t = TempDir::new("trash");
    let mut sh = ShellCom::new().unwrap();
    let a = t.file("victim.txt", "keep me");
    let out = sh.file_op(&FileOp::Trash(vec![a.clone()]));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 1);
    let (from, recycled) = out.items[0].clone();
    assert_eq!(from, a);
    assert!(!t.has("victim.txt"));
    if recycled == a {
        return;
    }
    let restored = sh.file_op(&FileOp::Restore(vec![(recycled.clone(), a.clone())]));
    if !t.has("victim.txt") {
        let _ = std::fs::remove_file(&*recycled.display());
        let _ = std::fs::remove_file(info_file(&recycled));
        panic!("restore failed: {:?}", restored.error);
    }
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.items, vec![(recycled.clone(), a.clone())]);
    assert_eq!(std::fs::read_to_string(t.0.join("victim.txt")).unwrap(), "keep me");
    assert!(unsafe { recycled_items(std::slice::from_ref(&recycled)) }.unwrap()[0].is_none(), "the Recycle Bin no longer lists the item");
    let _ = std::fs::remove_file(info_file(&recycled));
}

#[test]
fn same_volume_moves_renames_and_new_folders_skip_the_shell_engine() {
    let t = TempDir::new("direct");
    std::fs::create_dir(t.0.join("sub")).unwrap();
    let (a, b) = (t.file("a.txt", "a"), t.file("b.txt", "b"));
    let (done, rest) = direct(&FileOp::Move { items: vec![a.clone(), b.clone()], dest: t.path("sub") });
    assert_eq!(rest, None);
    assert_eq!(done, vec![(a, t.path("sub/a.txt")), (b, t.path("sub/b.txt"))]);
    assert!(t.has("sub/a.txt") && !t.has("a.txt"));
    let (done, rest) = direct(&FileOp::Rename { item: t.path("sub/a.txt"), name: b"c.txt".to_vec() });
    assert_eq!((done, rest), (vec![(t.path("sub/a.txt"), t.path("sub/c.txt"))], None));
    let (done, rest) = direct(&FileOp::MoveTo(vec![(t.path("sub/c.txt"), t.path("a.txt"))]));
    assert_eq!((done, rest), (vec![(t.path("sub/c.txt"), t.path("a.txt"))], None));
    let (done, rest) = direct(&FileOp::NewFolder { dir: t.path("sub"), name: b"New folder".to_vec() });
    assert_eq!((done, rest), (vec![(t.path("sub/New folder"), t.path("sub/New folder"))], None));
    assert!(t.0.join("sub/New folder").is_dir());
}

#[test]
fn collisions_leave_the_rest_to_the_shell_engine() {
    let t = TempDir::new("collide");
    std::fs::create_dir_all(t.0.join("sub/taken")).unwrap();
    let (a, b, c) = (t.file("a.txt", "a"), t.file("b.txt", "b"), t.file("c.txt", "c"));
    t.file("sub/b.txt", "old");
    let (done, rest) = direct(&FileOp::Move { items: vec![a.clone(), b.clone(), c.clone()], dest: t.path("sub") });
    assert_eq!(done, vec![(a, t.path("sub/a.txt"))]);
    assert_eq!(rest, Some(FileOp::MoveTo(vec![(b.clone(), t.path("sub/b.txt")), (c.clone(), t.path("sub/c.txt"))])));
    assert!(t.has("b.txt") && t.has("c.txt"));
    let rename = FileOp::Rename { item: b, name: b"c.txt".to_vec() };
    assert_eq!(direct(&rename), (Vec::new(), Some(rename.clone())));
    let folder = FileOp::NewFolder { dir: t.path("sub"), name: b"taken".to_vec() };
    assert_eq!(direct(&folder), (Vec::new(), Some(folder.clone())));
    let trash = FileOp::Trash(vec![c]);
    assert_eq!(direct(&trash), (Vec::new(), Some(trash.clone())));
}
