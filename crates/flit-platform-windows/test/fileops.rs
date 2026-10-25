use super::*;
use flit_platform::Shell;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        Self::under(&std::env::temp_dir(), tag)
    }
    fn under(root: &Path, tag: &str) -> Self {
        let dir = root.join(format!("flit-ops-{tag}-{}", std::process::id()));
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
    assert!(!Path::new(&info_file(&recycled)).exists(), "the direct restore drops the $I file");
}

#[test]
fn restore_into_a_missing_folder_falls_back_to_the_recycle_bin_verb() {
    let t = TempDir::new("restore-gone");
    let mut sh = ShellCom::new().unwrap();
    std::fs::create_dir(t.0.join("sub")).unwrap();
    let a = t.file(r"sub\victim.txt", "keep me");
    let out = sh.file_op(&FileOp::Trash(vec![a.clone()]));
    assert!(out.error.is_none(), "{:?}", out.error);
    let recycled = out.items[0].1.clone();
    if recycled == a {
        return;
    }
    std::fs::remove_dir(t.0.join("sub")).unwrap();
    let pair = (recycled.clone(), a.clone());
    assert_eq!(direct(&FileOp::Restore(vec![pair.clone()])), (Vec::new(), Some(FileOp::Restore(vec![pair.clone()]))));
    let restored = sh.file_op(&FileOp::Restore(vec![pair.clone()]));
    let _ = std::fs::remove_file(info_file(&recycled));
    if !t.has(r"sub\victim.txt") {
        let _ = std::fs::remove_file(&*recycled.display());
        panic!("restore failed: {:?}", restored.error);
    }
    assert_eq!((restored.error.is_none(), restored.items), (true, vec![pair]));
}

#[test]
fn small_file_copies_skip_the_shell_engine_until_a_collision() {
    let t = TempDir::new("copy");
    std::fs::create_dir(t.0.join("sub")).unwrap();
    let (a, b, c) = (t.file("a.txt", "a"), t.file("b.txt", "b"), t.file("c.txt", "c"));
    t.file(r"sub\b.txt", "old");
    let (done, rest) = direct(&FileOp::Copy { items: vec![a.clone(), b.clone(), c.clone()], dest: t.path("sub") });
    assert_eq!(done, vec![(a.clone(), t.path(r"sub\a.txt"))]);
    assert_eq!(rest, Some(FileOp::Copy { items: vec![b.clone(), c], dest: t.path("sub") }));
    assert_eq!(std::fs::read_to_string(t.0.join(r"sub\a.txt")).unwrap(), "a");
    assert_eq!(std::fs::read_to_string(t.0.join(r"sub\b.txt")).unwrap(), "old");
    assert!(t.has("a.txt") && !t.has(r"sub\c.txt"));
    let duplicate = FileOp::Copy { items: vec![b], dest: t.path("") };
    assert_eq!(direct(&duplicate), (Vec::new(), Some(duplicate.clone())), "copies into the source folder keep the shell's \"- Copy\" names");
    let folder = FileOp::Copy { items: vec![t.path("sub")], dest: t.path("") };
    assert_eq!(direct(&folder), (Vec::new(), Some(folder.clone())));
}

#[test]
fn small_files_move_across_volumes_without_the_shell_engine() {
    let t = TempDir::new("xvol");
    let other = TempDir::under(&Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\target"), "xvol");
    if t.0.components().next() == other.0.components().next() {
        eprintln!("skipped: {} and {} share a volume", t.0.display(), other.0.display());
        return;
    }
    let a = t.file("a.txt", "across");
    let (done, rest) = direct(&FileOp::Move { items: vec![a.clone()], dest: other.path("") });
    assert_eq!((done, rest), (vec![(a, other.path("a.txt"))], None));
    assert_eq!(std::fs::read_to_string(other.0.join("a.txt")).unwrap(), "across");
    assert!(!t.has("a.txt"));

    let held = t.file("held.txt", "held");
    let lock = std::fs::OpenOptions::new().read(true).share_mode(1).open(t.0.join("held.txt")).unwrap();
    let blocked = FileOp::Move { items: vec![held], dest: other.path("") };
    assert_eq!(direct(&blocked), (Vec::new(), Some(blocked.clone())), "a source that cannot be deleted is left to the shell engine");
    drop(lock);
    assert!(t.has("held.txt") && !other.has("held.txt"));

    std::fs::create_dir(t.0.join("sub")).unwrap();
    t.file(r"sub\f.txt", "f");
    let folder = FileOp::Move { items: vec![t.path("sub")], dest: other.path("") };
    assert_eq!(direct(&folder), (Vec::new(), Some(folder.clone())));
    assert!(t.has(r"sub\f.txt"));
}

#[test]
fn the_quiet_budget_counts_entries_and_bytes() {
    let t = TempDir::new("budget");
    std::fs::create_dir(t.0.join("few")).unwrap();
    (0..3).for_each(|i| drop(t.file(&format!(r"few\{i}"), "x")));
    let small_set = vec![t.file("a.txt", "a"), t.path("few")];
    let budget = || (QUIET_ENTRIES, QUIET_BYTES);
    assert!(small_set.iter().all(|p| small(p, false, &mut budget())));
    assert!(!small(&t.path("few"), true, &mut budget()), "folders are not plain files");
    assert!(quiet(&FileOp::Trash(small_set)) && quiet(&FileOp::NewFolder { dir: t.path(""), name: b"x".to_vec() }));
    std::fs::File::create(t.0.join("big")).unwrap().set_len(QUIET_BYTES + 1).unwrap();
    assert!(!quiet(&FileOp::Trash(vec![t.path("big")])));
    std::fs::create_dir(t.0.join("many")).unwrap();
    (0..QUIET_ENTRIES).for_each(|i| drop(t.file(&format!(r"many\{i}"), "")));
    assert!(!quiet(&FileOp::Copy { items: vec![t.path("many")], dest: t.path("few") }));
    assert!(!quiet(&FileOp::Trash(vec![t.path("missing")])));
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

#[test]
fn permanent_deletion_neither_recycles_nor_skips_the_windows_confirmation() {
    let t = TempDir::new("flags");
    let a = t.file("a.txt", "x");
    let delete = flags(&FileOp::Delete(vec![a.clone()]));
    assert_eq!(delete.0 & (FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0 | FOF_NOCONFIRMATION.0), 0);
    assert_ne!(flags(&FileOp::Trash(vec![a.clone()])).0 & FOFX_RECYCLEONDELETE.0, 0);
    assert_eq!(cancelled(&FileOp::Delete(vec![a])).to_string(), cancelled(&FileOp::EmptyBin).to_string());
}
