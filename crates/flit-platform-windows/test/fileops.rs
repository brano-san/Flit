use super::*;
use flit_core::provider::{Conflict, OpCtl, Resolve};
use flit_platform::{Shell, CANCELLED};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

fn run(sh: &mut ShellCom, op: &FileOp) -> OpOutcome {
    sh.file_op(op, &Job::detached(Resolve::KeepBoth))
}

fn read(t: &TempDir, rel: &str) -> String {
    std::fs::read_to_string(t.0.join(rel)).unwrap()
}

type Asked = Arc<Mutex<Vec<Conflict>>>;

fn answering(answers: &[(Resolve, bool)]) -> (Job, Asked) {
    let ctl = OpCtl::new(None);
    let left = Mutex::new(answers.to_vec());
    let asked: Asked = Arc::default();
    let (seen, answer) = (asked.clone(), ctl.clone());
    let conflict = move |c: Conflict| {
        seen.lock().unwrap().push(c);
        let mut left = left.lock().unwrap();
        match left.is_empty() {
            true => answer.cancel(),
            false => {
                let (choice, all) = left.remove(0);
                answer.answer(choice, all);
            }
        }
    };
    (Job::new(ctl, Box::new(|_| {}), Box::new(conflict)), asked)
}

fn other_volume(tag: &str) -> Option<TempDir> {
    let other = TempDir::under(&Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\target"), tag);
    let temp = std::env::temp_dir();
    if temp.components().next() == other.0.components().next() {
        eprintln!("skipped: {} and {} share a volume", temp.display(), other.0.display());
        return None;
    }
    Some(other)
}

#[test]
fn copy_move_rename_new_folder_and_link_report_their_results() {
    let t = TempDir::new("basic");
    let mut sh = ShellCom::new().unwrap();
    let a = t.file("a.txt", "alpha");
    std::fs::create_dir(t.0.join("sub")).unwrap();

    let out = run(&mut sh, &FileOp::Copy { items: vec![a.clone()], dest: t.path("sub") });
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, vec![(a.clone(), t.path(r"sub/a.txt"))]);
    assert_eq!(std::fs::read_to_string(t.0.join(r"sub/a.txt")).unwrap(), "alpha");

    let out = run(&mut sh, &FileOp::Copy { items: vec![a.clone()], dest: t.path("") });
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 1);
    assert_ne!(out.items[0].1, a, "a copy into the same folder gets a new name");
    assert!(Path::new(&*out.items[0].1.display()).exists());

    let out = run(&mut sh, &FileOp::Rename { item: a.clone(), name: b"b.txt".to_vec() });
    assert_eq!((out.error.is_none(), out.items), (true, vec![(a.clone(), t.path("b.txt"))]));
    assert!(t.has("b.txt") && !t.has("a.txt"));

    let out = run(&mut sh, &FileOp::NewFolder { dir: t.path(""), name: b"New folder".to_vec() });
    assert_eq!(names(&out.items), ["New folder"]);
    let out = run(&mut sh, &FileOp::NewFolder { dir: t.path(""), name: b"New folder".to_vec() });
    assert_eq!(names(&out.items), ["New folder (2)"], "a taken name is renamed instead of failing");

    let out = run(&mut sh, &FileOp::Move { items: vec![t.path("b.txt")], dest: t.path("New folder") });
    assert_eq!(out.items, vec![(t.path("b.txt"), t.path(r"New folder\b.txt"))]);
    let out = run(&mut sh, &FileOp::MoveTo(vec![(t.path(r"New folder\b.txt"), t.path("a.txt"))]));
    assert_eq!(out.items, vec![(t.path(r"New folder\b.txt"), t.path("a.txt"))]);
    assert!(t.has("a.txt") && !t.has(r"New folder\b.txt"));

    let out = run(&mut sh, &FileOp::Link { items: vec![t.path("a.txt")], dest: t.path("sub") });
    assert_eq!(names(&out.items), ["a.txt - Shortcut.lnk"]);
    let out = run(&mut sh, &FileOp::Link { items: vec![t.path("a.txt")], dest: t.path("sub") });
    assert_eq!(names(&out.items), ["a.txt - Shortcut (2).lnk"]);
}

#[test]
fn new_files_come_from_templates_and_streams_attach_to_files() {
    let t = TempDir::new("newfile");
    let mut sh = ShellCom::new().unwrap();
    let source = t.file("source.docx", "template body");
    std::fs::File::options().write(true).open(t.0.join("source.docx")).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH).unwrap();
    let new = |name: &str, template| FileOp::NewFile { dir: t.path(""), name: name.as_bytes().to_vec(), template };
    let out = run(&mut sh, &new("New Text Document.txt", Template::Empty));
    assert_eq!((out.error.is_none(), names(&out.items)), (true, vec!["New Text Document.txt".to_owned()]));
    assert_eq!(std::fs::metadata(t.0.join("New Text Document.txt")).unwrap().len(), 0);
    let out = run(&mut sh, &new("New Text Document.txt", Template::Empty));
    assert_eq!(names(&out.items), ["New Text Document (2).txt"], "a taken name is renamed instead of overwritten");
    run(&mut sh, &new("New.zip", Template::Data(vec![0x50, 0x4B, 5, 6])));
    assert_eq!(std::fs::read(t.0.join("New.zip")).unwrap(), [0x50, 0x4B, 5, 6]);
    let out = run(&mut sh, &new("New.docx", Template::Copy(source.clone())));
    assert_eq!(out.items, vec![(t.path("New.docx"), t.path("New.docx"))]);
    assert_eq!(std::fs::read_to_string(t.0.join("New.docx")).unwrap(), "template body");
    let modified = std::fs::metadata(t.0.join("New.docx")).unwrap().modified().unwrap();
    assert!(modified.elapsed().unwrap() < std::time::Duration::from_secs(600), "a copied template is dated now");
    let taken = new("New.docx", Template::Empty);
    assert_eq!(direct(&taken), (Vec::new(), Some(taken.clone())));
    assert_eq!(std::fs::read_to_string(t.0.join("New.docx")).unwrap(), "template body");

    let a = t.file("a.txt", "main");
    let out = run(&mut sh, &FileOp::NewStream { item: a.clone(), name: b"meta".to_vec() });
    assert_eq!((out.error.is_none(), out.items), (true, vec![(a.clone(), a.clone())]));
    assert!(std::fs::metadata(format!("{}:meta", t.0.join("a.txt").display())).is_ok());
    assert_eq!(std::fs::read_to_string(t.0.join("a.txt")).unwrap(), "main");
    let again = run(&mut sh, &FileOp::NewStream { item: a, name: b"meta".to_vec() });
    assert!(again.items.is_empty() && again.error.unwrap().message.starts_with("Cannot create the stream"));

    let out = run(&mut sh, &FileOp::Link { items: vec![t.path("missing.txt")], dest: t.path("") });
    assert!(out.items.is_empty() && out.error.unwrap().message.ends_with("does not exist"));
}

#[test]
fn a_folder_icon_lands_in_desktop_ini_and_the_shell_draws_it() {
    use flit_core::ui::{IconKind, IconRequest};
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM};
    let t = TempDir::new("folder-icon");
    std::fs::create_dir(t.0.join("photos")).unwrap();
    let (folder, icon) = (t.path("photos"), FsPath::parse(r"C:\Windows\System32\shell32.dll").unwrap());
    let mut sh = ShellCom::new().unwrap();
    let out = run(&mut sh, &FileOp::FolderIcon { folder: folder.clone(), icon: Some(icon.clone()), index: 3 });
    assert_eq!((out.error.map(|e| e.message), out.items), (None, vec![(folder.clone(), icon)]));
    assert!(crate::shell::desktop_ini(&folder).unwrap().icon);
    let attrs = std::fs::metadata(t.0.join("photos")).unwrap().file_attributes();
    assert_ne!(attrs & (FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_SYSTEM.0), 0, "Explorer reads desktop.ini only in read-only or system folders");
    let req = IconRequest { key: 1, kind: IconKind::CustomFolder, path: folder.clone(), size: 32, gen: 0, row: 0 };
    assert!(sh.icon(&req).is_some_and(|b| b.width > 0 && b.rgba.chunks_exact(4).any(|p| p[3] != 0)));
    let out = run(&mut sh, &FileOp::FolderIcon { folder: folder.clone(), icon: None, index: 0 });
    assert_eq!((out.error.map(|e| e.message), out.items), (None, vec![(folder.clone(), folder.clone())]));
    assert!(!crate::shell::desktop_ini(&folder).is_some_and(|d| d.icon), "restoring the default icon removes it from desktop.ini");
}

#[test]
fn unaddressable_names_are_refused_before_the_shell_sees_them() {
    let t = TempDir::new("refuse");
    let mut sh = ShellCom::new().unwrap();
    let a = t.file("a.txt", "x");
    let out = run(&mut sh, &FileOp::Rename { item: a.clone(), name: b"b.".to_vec() });
    assert!(out.error.is_some() && out.items.is_empty());
    let out = run(&mut sh, &FileOp::Copy { items: vec![t.path("trailing.")], dest: t.path("") });
    assert!(out.error.unwrap().message.contains("dot or a space"));
    assert!(t.has("a.txt"));
}

#[test]
fn trash_reports_the_recycled_item_and_restore_brings_it_back() {
    let t = TempDir::new("trash");
    let mut sh = ShellCom::new().unwrap();
    let a = t.file("victim.txt", "keep me");
    let out = run(&mut sh, &FileOp::Trash(vec![a.clone()]));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 1);
    let (from, recycled) = out.items[0].clone();
    assert_eq!(from, a);
    assert!(!t.has("victim.txt"));
    if recycled == a {
        return;
    }
    let restored = run(&mut sh, &FileOp::Restore(vec![(recycled.clone(), a.clone())]));
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
    let out = run(&mut sh, &FileOp::Trash(vec![a.clone()]));
    assert!(out.error.is_none(), "{:?}", out.error);
    let recycled = out.items[0].1.clone();
    if recycled == a {
        return;
    }
    std::fs::remove_dir(t.0.join("sub")).unwrap();
    let pair = (recycled.clone(), a.clone());
    assert_eq!(direct(&FileOp::Restore(vec![pair.clone()])), (Vec::new(), Some(FileOp::Restore(vec![pair.clone()]))));
    let restored = run(&mut sh, &FileOp::Restore(vec![pair.clone()]));
    let _ = std::fs::remove_file(info_file(&recycled));
    if !t.has(r"sub\victim.txt") {
        let _ = std::fs::remove_file(&*recycled.display());
        panic!("restore failed: {:?}", restored.error);
    }
    assert_eq!((restored.error.is_none(), restored.items), (true, vec![pair]));
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
fn permanent_deletion_neither_recycles_nor_skips_the_windows_confirmation() {
    let t = TempDir::new("flags");
    let a = t.file("a.txt", "x");
    let delete = flags(&FileOp::Delete(vec![a.clone()]));
    assert_eq!(delete.0 & (FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0 | FOF_NOCONFIRMATION.0), 0);
    assert_ne!(flags(&FileOp::Trash(vec![a.clone()])).0 & FOFX_RECYCLEONDELETE.0, 0);
    assert_eq!(cancelled(&FileOp::Delete(vec![a])).to_string(), cancelled(&FileOp::EmptyBin).to_string());
}

#[test]
fn no_operation_silences_windows_confirmations_or_errors() {
    let t = TempDir::new("noflags");
    let a = t.file("a.txt", "x");
    for op in [FileOp::Trash(vec![a.clone()]), FileOp::Delete(vec![a.clone()]), FileOp::Copy { items: vec![a.clone()], dest: t.path("") }, FileOp::MoveTo(vec![(a.clone(), t.path("b.txt"))])] {
        assert_eq!(flags(&op).0 & (FOF_NOCONFIRMATION.0 | FOF_NOERRORUI.0), 0, "{op:?}");
    }
    assert_ne!(flags(&FileOp::Trash(vec![a])).0 & FOF_SILENT.0, 0, "Flit draws the progress of a trash operation itself");
}

#[test]
fn a_copy_short_of_space_asks_before_it_fails() {
    let t = TempDir::new("room");
    std::fs::create_dir(t.0.join("dst")).unwrap();
    let (job, asked) = answering(&[(Resolve::Skip, false)]);
    assert!(engine::Engine::new(&job).room(&t.path("dst"), u64::MAX), "copy anyway goes ahead");
    let seen = asked.lock().unwrap().clone();
    assert!(seen.len() == 1 && flit_core::limits::is_short_of_space(&seen[0]) && seen[0].from.size == u64::MAX);
    let (job, _) = answering(&[]);
    assert!(!engine::Engine::new(&job).room(&t.path("dst"), u64::MAX), "cancel stops the copy");
    let (job, asked) = answering(&[]);
    assert!(engine::Engine::new(&job).room(&t.path("dst"), 1) && asked.lock().unwrap().is_empty());
    assert!(engine::volume_file_system(&t.path("dst")).is_some_and(|fs| !fs.is_empty()));
}

#[test]
fn copies_trees_natively_and_names_duplicates_like_explorer() {
    let t = TempDir::new("tree");
    std::fs::create_dir_all(t.0.join(r"src\sub\deep")).unwrap();
    std::fs::create_dir(t.0.join("dst")).unwrap();
    t.file(r"src\a.txt", "a");
    t.file(r"src\sub\deep\c.txt", "c");
    let job = Job::detached(Resolve::Skip);
    let out = engine::run(&FileOp::Copy { items: vec![t.path("src")], dest: t.path("dst") }, &job);
    assert!(out.error.is_none() && out.rest.is_none(), "{:?}", out.error);
    assert_eq!(out.items, vec![(t.path("src"), t.path(r"dst\src"))], "a new folder is one undo item");
    assert_eq!((read(&t, r"dst\src\a.txt"), read(&t, r"dst\src\sub\deep\c.txt")), ("a".into(), "c".into()));
    assert!(t.has(r"src\sub\deep\c.txt"));
    let p = job.progress();
    assert_eq!((p.total_items, p.items, p.total_bytes, p.bytes), (5, 5, 2, 2));

    let mut sh = ShellCom::new().unwrap();
    let a = t.path(r"src\a.txt");
    assert_eq!(names(&run(&mut sh, &FileOp::Copy { items: vec![a.clone()], dest: t.path("src") }).items), ["a - Copy.txt"]);
    assert_eq!(names(&run(&mut sh, &FileOp::Copy { items: vec![a], dest: t.path("src") }).items), ["a - Copy (2).txt"]);
    assert_eq!(names(&run(&mut sh, &FileOp::Copy { items: vec![t.path(r"src\sub")], dest: t.path("src") }).items), ["sub - Copy"]);

    let into_itself = run(&mut sh, &FileOp::Copy { items: vec![t.path("src")], dest: t.path(r"src\sub") });
    assert!(into_itself.items.is_empty() && into_itself.error.unwrap().message.contains("inside the source folder"));
    assert!(!t.has(r"src\sub\src"));

    let mut engine = engine::Engine::new(&job);
    assert!(engine.room(&t.path("dst"), 1) && !engine.room(&t.path("dst"), u64::MAX), "a copy larger than the free space is refused before it starts");
}

#[test]
fn conflicts_keep_both_replace_or_skip_and_never_touch_unchosen_items() {
    let t = TempDir::new("conflict");
    std::fs::create_dir_all(t.0.join("src")).unwrap();
    std::fs::create_dir_all(t.0.join("dst")).unwrap();
    let items: Vec<FsPath> = ["x.txt", "y.txt", "z.txt"].iter().map(|n| t.file(&format!(r"src\{n}"), "new")).collect();
    ["x.txt", "y.txt", "z.txt"].iter().for_each(|n| drop(t.file(&format!(r"dst\{n}"), "old!")));
    let (job, asked) = answering(&[(Resolve::KeepBoth, false), (Resolve::Replace, false), (Resolve::Skip, false)]);
    let out = engine::run(&FileOp::Copy { items: items.clone(), dest: t.path("dst") }, &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, vec![(items[0].clone(), t.path(r"dst\x (2).txt")), (items[1].clone(), t.path(r"dst\y.txt"))]);
    assert_eq!([read(&t, r"dst\x.txt"), read(&t, r"dst\x (2).txt"), read(&t, r"dst\y.txt"), read(&t, r"dst\z.txt")], ["old!", "new", "new", "old!"]);
    let seen = asked.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert_eq!((seen[0].source.clone(), seen[0].target.clone(), seen[0].from.size, seen[0].to.size), (items[0].clone(), t.path(r"dst\x.txt"), 3, 4));
    assert!(!t.has(r"dst\y.txt.flit-partial"));

    let (job, asked) = answering(&[(Resolve::Skip, true)]);
    let out = engine::run(&FileOp::Copy { items: items.clone(), dest: t.path("dst") }, &job);
    assert_eq!((out.items.len(), asked.lock().unwrap().len(), out.error.is_none()), (0, 1, true), "the answer applies to all remaining conflicts");

    let mut ro = std::fs::metadata(t.0.join(r"dst\z.txt")).unwrap().permissions();
    ro.set_readonly(true);
    std::fs::set_permissions(t.0.join(r"dst\z.txt"), ro).unwrap();
    let out = engine::run(&FileOp::Copy { items: vec![items[2].clone()], dest: t.path("dst") }, &Job::detached(Resolve::Replace));
    assert_eq!((out.items.len(), read(&t, r"dst\z.txt")), (1, "new".into()), "{:?}", out.error);
}

#[test]
fn many_small_files_copy_in_parallel_and_still_ask_about_every_conflict_in_order() {
    let t = TempDir::new("quick");
    std::fs::create_dir_all(t.0.join(r"src\tree")).unwrap();
    std::fs::create_dir_all(t.0.join(r"dst\tree")).unwrap();
    let names: Vec<String> = (0..30).map(|k| format!("f{k:02}.txt")).collect();
    let items: Vec<FsPath> = names.iter().map(|n| t.file(&format!(r"src\{n}"), "new")).collect();
    names.iter().for_each(|n| drop(t.file(&format!(r"src\tree\{n}"), "new")));
    for k in [3, 11, 27] {
        drop(t.file(&format!(r"dst\{}", names[k]), "old!"));
        drop(t.file(&format!(r"dst\tree\{}", names[k]), "old!"));
    }
    let (job, asked) = answering(&[(Resolve::Skip, false), (Resolve::Replace, false), (Resolve::KeepBoth, false)]);
    let out = engine::run(&FileOp::Copy { items: items.clone(), dest: t.path("dst") }, &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    let targets: Vec<FsPath> = asked.lock().unwrap().iter().map(|c| c.target.clone()).collect();
    assert_eq!(targets, [3, 11, 27].map(|k| t.path(&format!(r"dst\{}", names[k]))));
    assert_eq!(out.items.len(), 29);
    assert_eq!([read(&t, r"dst\f03.txt"), read(&t, r"dst\f11.txt"), read(&t, r"dst\f27.txt"), read(&t, r"dst\f27 (2).txt"), read(&t, r"dst\f00.txt")], ["old!", "new", "old!", "new", "new"]);
    assert!(!t.has(r"dst\f11.txt.flit-partial"));

    let (job, asked) = answering(&[(Resolve::Replace, false), (Resolve::Skip, true)]);
    let out = engine::run(&FileOp::Copy { items: vec![t.path(r"src\tree")], dest: t.path("dst") }, &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(asked.lock().unwrap().len(), 2, "the folder, then one file with Skip for all");
    assert_eq!(out.items.len(), 27);
    assert_eq!([read(&t, r"dst\tree\f03.txt"), read(&t, r"dst\tree\f00.txt"), read(&t, r"dst\tree\f29.txt")], ["old!", "new", "new"]);
}

#[test]
fn replacing_a_folder_merges_and_keeps_what_only_the_target_has() {
    let t = TempDir::new("merge");
    std::fs::create_dir_all(t.0.join(r"src\f")).unwrap();
    std::fs::create_dir_all(t.0.join(r"dst\f")).unwrap();
    std::fs::create_dir_all(t.0.join(r"dst\g")).unwrap();
    t.file(r"src\f\a", "new");
    t.file(r"src\f\b", "new");
    t.file(r"src\g", "file");
    t.file(r"dst\f\a", "old");
    t.file(r"dst\f\keep", "old");
    let (job, _) = answering(&[(Resolve::Replace, false), (Resolve::Replace, false), (Resolve::Replace, false)]);
    let out = engine::run(&FileOp::Move { items: vec![t.path(r"src\f"), t.path(r"src\g")], dest: t.path("dst") }, &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    let mut items = out.items;
    items.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
    assert_eq!(items, vec![(t.path(r"src\f\a"), t.path(r"dst\f\a")), (t.path(r"src\f\b"), t.path(r"dst\f\b")), (t.path(r"src\g"), t.path(r"dst\g (2)"))]);
    assert_eq!([read(&t, r"dst\f\a"), read(&t, r"dst\f\b"), read(&t, r"dst\f\keep"), read(&t, r"dst\g (2)")], ["new", "new", "old", "file"]);
    assert!(t.0.join(r"dst\g").is_dir(), "a folder is never replaced by a file");
    assert!(!t.has(r"src\f"), "a fully merged source folder is removed");
}

#[test]
fn moves_across_volumes_delete_a_source_only_after_its_copy() {
    let Some(other) = other_volume("xvol") else { return };
    let t = TempDir::new("xvol");
    std::fs::create_dir_all(t.0.join(r"m\sub")).unwrap();
    t.file(r"m\a.txt", "a");
    t.file(r"m\sub\b.txt", "b");
    let mut sh = ShellCom::new().unwrap();
    let out = run(&mut sh, &FileOp::Move { items: vec![t.path("m")], dest: other.path("") });
    assert_eq!((out.error.is_none(), out.items), (true, vec![(t.path("m"), other.path("m"))]));
    assert!(!t.has("m") && other.has(r"m\sub\b.txt"));

    let held = t.file("held.txt", "held");
    let lock = std::fs::OpenOptions::new().read(true).share_mode(1).open(t.0.join("held.txt")).unwrap();
    let out = run(&mut sh, &FileOp::Move { items: vec![held], dest: other.path("") });
    drop(lock);
    assert!(out.items.is_empty() && out.error.is_some());
    assert!(t.has("held.txt") && !other.has("held.txt"), "a source that cannot be deleted keeps no second copy");
}

#[test]
fn cancelling_leaves_the_remaining_sources_in_place() {
    let Some(other) = other_volume("cancel") else { return };
    let t = TempDir::new("cancel");
    let items: Vec<FsPath> = ["a", "b", "c"].iter().map(|n| t.file(n, n)).collect();
    other.file("b", "old");
    let (job, _) = answering(&[]);
    let out = engine::run(&FileOp::Move { items: items.clone(), dest: other.path("") }, &job);
    assert_eq!(out.error.map(|e| e.message), Some(CANCELLED.into()));
    assert_eq!((out.items, out.rest), (vec![(items[0].clone(), other.path("a"))], None));
    assert!(!t.has("a") && t.has("b") && t.has("c") && !other.has("c"));
    assert_eq!(std::fs::read_to_string(other.0.join("b")).unwrap(), "old");

    let big = t.path("big");
    std::fs::File::create(t.0.join("big")).unwrap().set_len(8 << 20).unwrap();
    other.file("big", "old");
    let stopped = Job::detached(Resolve::Replace);
    stopped.ctl().cancel();
    let engine = engine::Engine::new(&stopped);
    assert!(engine.copy_file(&big, &other.path("big"), 8 << 20, true).is_err());
    assert!(engine.copy_file(&big, &other.path("fresh"), 8 << 20, false).is_err());
    assert_eq!(std::fs::read_to_string(other.0.join("big")).unwrap(), "old", "a cancelled replacement keeps the old file");
    assert!(!other.has("fresh") && !other.has("big.flit-partial"));
}

#[test]
fn delete_clears_read_only_trees_and_reports_top_items() {
    let t = TempDir::new("delete");
    std::fs::create_dir_all(t.0.join(r"d\sub")).unwrap();
    let read_only = |rel: &str| {
        let mut p = std::fs::metadata(t.0.join(rel)).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(t.0.join(rel), p).unwrap();
    };
    t.file(r"d\sub\x", "x");
    t.file(r"d\ro.txt", "r");
    t.file("f.txt", "f");
    read_only(r"d\ro.txt");
    read_only("f.txt");
    let job = Job::detached(Resolve::Skip);
    let out = engine::run(&FileOp::Delete(vec![t.path("d"), t.path("f.txt")]), &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, vec![(t.path("d"), t.path("d")), (t.path("f.txt"), t.path("f.txt"))]);
    assert!(!t.has("d") && !t.has("f.txt"));
    assert_eq!(job.progress().items, 5);
}

#[test]
fn flatten_moves_nested_files_up_asks_on_collisions_and_undoes() {
    let t = TempDir::new("flatten");
    std::fs::create_dir_all(t.0.join(r"root\a\b")).unwrap();
    std::fs::create_dir_all(t.0.join(r"root\c")).unwrap();
    t.file(r"root\top.txt", "root");
    t.file(r"root\a\one.txt", "1");
    t.file(r"root\a\b\two.txt", "2");
    t.file(r"root\c\top.txt", "c");
    let (job, asked) = answering(&[(Resolve::KeepBoth, false)]);
    let out = engine::run(&FileOp::Flatten(vec![t.path("root")]), &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    let mut moved = out.items.clone();
    moved.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
    assert_eq!(moved, vec![(t.path(r"root\a\one.txt"), t.path(r"root\one.txt")), (t.path(r"root\c\top.txt"), t.path(r"root\top (2).txt")), (t.path(r"root\a\b\two.txt"), t.path(r"root\two.txt"))]);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(!t.has(r"root\a") && !t.has(r"root\c") && read(&t, r"root\top.txt") == "root");

    let report = flit_core::provider::OpReport { op: FileOp::Flatten(vec![t.path("root")]), items: out.items };
    let Some(flit_core::undo::Step::Run(undo)) = flit_core::undo::inverse(&report) else { panic!("flatten is undoable") };
    let back = engine::run(&undo, &Job::detached(Resolve::Skip));
    assert!(back.error.is_none(), "{:?}", back.error);
    assert_eq!([read(&t, r"root\a\one.txt"), read(&t, r"root\a\b\two.txt"), read(&t, r"root\c\top.txt")], ["1", "2", "c"]);
}

#[test]
fn archives_are_folders_for_listing_and_operations() {
    use flit_core::archive::{ArchiveOp, Compression, Format, Place};
    use flit_core::ListingBuilder;
    use flit_platform::Fs;
    let t = TempDir::new("archive");
    std::fs::create_dir_all(t.0.join(r"src\docs")).unwrap();
    t.file(r"src\docs\a.txt", "alpha");
    t.file(r"src\b.txt", "beta");
    let mut sh = ShellCom::new().unwrap();
    let zip = t.path("pack.zip");
    let options = Compression { format: Format::Zip, level: 6, ..Compression::default() };
    let made = run(&mut sh, &FileOp::Archive(ArchiveOp::Compress { items: vec![t.path(r"src\docs"), t.path(r"src\b.txt")], archive: zip.clone(), options }));
    assert_eq!(made.items, vec![(t.path(r"src\docs"), zip.clone())]);
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Sensitive, &mut go);
    crate::NtFs.read_dir(&zip.join(b"docs"), &mut b).unwrap();
    assert_eq!((b.listing.len(), b.listing.name(0), b.listing.entries[0].attrs & flit_core::listing::attr::ARCHIVED != 0), (1, &b"a.txt"[..], true));
    let copied = run(&mut sh, &FileOp::Copy { items: vec![zip.join(b"docs").join(b"a.txt")], dest: t.path("out") });
    assert!(copied.error.is_none(), "{:?}", copied.error);
    assert_eq!(read(&t, r"out\a.txt"), "alpha");
    let refused = run(&mut sh, &FileOp::Rename { item: zip.join(b"b.txt"), name: b"c.txt".to_vec() });
    assert_eq!(refused.error.map(|e| e.message), Some(flit_core::i18n::S::ErrorsArchiveInside.en().to_owned()));
    let here = run(&mut sh, &FileOp::Archive(ArchiveOp::Extract { archives: vec![zip.clone()], dest: None, place: Place::OwnFolder }));
    assert_eq!(here.items, vec![(zip.clone(), t.path("pack"))]);
    assert_eq!(read(&t, r"pack\docs\a.txt"), "alpha");
}

#[test]
fn renames_refuse_taken_names_but_change_case() {
    let t = TempDir::new("rename");
    let a = t.file("a.txt", "a");
    t.file("b.txt", "b");
    let mut sh = ShellCom::new().unwrap();
    let taken = run(&mut sh, &FileOp::Rename { item: a.clone(), name: b"b.txt".to_vec() });
    assert!(taken.items.is_empty() && taken.error.unwrap().message.contains("already exists"));
    assert_eq!(read(&t, "b.txt"), "b");
    let cased = run(&mut sh, &FileOp::Rename { item: a.clone(), name: b"A.txt".to_vec() });
    assert_eq!(cased.items, vec![(a, t.path("A.txt"))]);
    let batch = run(&mut sh, &FileOp::MoveTo(vec![(t.path("A.txt"), t.path("Trip (1).txt")), (t.path("b.txt"), t.path("Trip (2).txt"))]));
    assert_eq!(names(&batch.items), ["Trip (1).txt", "Trip (2).txt"]);
}

#[test]
fn devices_refuse_what_the_shell_cannot_do_on_them() {
    let phone = FsPath::device(b"Pixel 7").unwrap().join(b"DCIM");
    let job = Job::detached(flit_core::provider::Resolve::Skip);
    for op in [FileOp::NewFile { dir: phone.clone(), name: b"a.txt".to_vec(), template: Template::Empty }, FileOp::Link { items: vec![phone.clone()], dest: FsPath::parse(r"C:\").unwrap() }] {
        let out = device_op(&op, &job);
        assert_eq!(out.error.map(|e| e.message), Some(S::ErrorsFsDeviceUnsupported.en().to_owned()));
    }
}
