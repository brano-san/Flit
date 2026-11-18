use super::*;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn l(s: &str) -> Location {
    Location::from_local(p(s))
}

#[derive(Default)]
struct Fake {
    pinned: Vec<FsPath>,
    stamp: u64,
    missing: Vec<FsPath>,
    partial: bool,
    broken: bool,
    no_stamp: bool,
    calls: Vec<String>,
}

impl Fake {
    fn with(pinned: &[&str]) -> Self {
        Fake { pinned: pinned.iter().map(|s| p(s)).collect(), stamp: 1, ..Fake::default() }
    }

    fn explorer(&mut self, pinned: &[&str]) {
        self.pinned = pinned.iter().map(|s| p(s)).collect();
        self.stamp += 1;
    }

    fn changes(&self) -> Vec<&str> {
        self.calls.iter().map(String::as_str).filter(|c| c.starts_with("pin ") || c.starts_with("unpin ")).collect()
    }
}

impl QuickAccess for Fake {
    fn pinned(&mut self) -> Option<Snapshot> {
        self.calls.push("read".into());
        (!self.broken).then(|| Snapshot { pinned: self.pinned.clone(), complete: !self.partial })
    }

    fn stamp(&mut self) -> Option<(u64, u128)> {
        self.calls.push("stamp".into());
        (!self.no_stamp).then_some((self.stamp, 0))
    }

    fn set_pinned(&mut self, folder: &FsPath, pin: bool) -> bool {
        self.calls.push(format!("{} {}", if pin { "pin" } else { "unpin" }, folder.display()));
        self.pinned.retain(|x| !x.same(folder));
        if pin {
            self.pinned.push(folder.clone());
        }
        self.stamp += 1;
        true
    }

    fn is_dir(&mut self, path: &FsPath) -> bool {
        !self.missing.iter().any(|m| m.same(path))
    }
}

fn running(quick: Fake) -> QuickSync<Fake> {
    let mut sync = QuickSync::new(quick);
    sync.resume();
    sync
}

#[test]
fn pinning_and_unpinning_a_folder_in_flit_does_the_same_in_quick_access() {
    let mut sync = running(Fake::with(&[r"C:\Work"]));
    sync.flit_changed(&[l(r"C:\Work")], &[l(r"C:\Work"), l(r"D:\Photos")]);
    assert_eq!(sync.quick().changes(), [r"pin D:\Photos"]);
    sync.flit_changed(&[l(r"C:\Work"), l(r"D:\Photos")], &[l(r"D:\Photos")]);
    assert_eq!(sync.quick().changes(), [r"pin D:\Photos", r"unpin C:\Work"]);
    assert_eq!(sync.quick().pinned, [p(r"D:\Photos")]);
}

#[test]
fn reordering_bookmarks_changes_nothing_in_quick_access() {
    let mut sync = running(Fake::with(&[r"C:\Work", r"D:\Photos"]));
    sync.flit_changed(&[l(r"C:\Work"), l(r"D:\Photos")], &[l(r"D:\Photos"), l(r"C:\Work")]);
    assert!(sync.quick().changes().is_empty());
    assert_eq!(pins_diff(&[l(r"C:\Work"), l(r"D:\Photos")], &[l(r"d:\photos"), l(r"C:\Work")]), PinsDiff::default());
}

#[test]
fn with_the_setting_off_quick_access_is_never_touched() {
    let mut sync = QuickSync::new(Fake::with(&[r"C:\Work"]));
    sync.flit_changed(&[], &[l(r"D:\Photos")]);
    sync.flit_changed(&[l(r"C:\Work")], &[]);
    assert_eq!(sync.poll(), None);
    assert!(sync.quick().calls.is_empty(), "{:?}", sync.quick().calls);
    let mut sync = running(Fake::with(&[r"C:\Work"]));
    sync.disable();
    sync.quick.calls.clear();
    sync.flit_changed(&[l(r"C:\Work")], &[]);
    assert_eq!(sync.poll(), None);
    assert!(sync.quick().calls.is_empty(), "{:?}", sync.quick().calls);
}

#[test]
fn turning_the_setting_on_merges_both_lists_once() {
    let mut sync = QuickSync::new(Fake::with(&[r"C:\Shared", r"E:\Music", r"\\nas\share"]));
    let flit = [l(r"C:\Work"), l(r"c:\shared"), l(r"\\wsl.localhost\Ubuntu\home"), l(r"\\nas\other")];
    let change = sync.enable(&flit);
    assert_eq!(sync.quick().changes(), [r"pin C:\Work"], "Flit bookmarks missing in Quick access are pinned there, network and WSL places are not");
    assert_eq!(change.added, [p(r"E:\Music")]);
    let merged = apply_incoming(&flit, &change).unwrap();
    assert_eq!(merged, [flit[0].clone(), flit[1].clone(), flit[2].clone(), flit[3].clone(), l(r"E:\Music")], "Quick access folders go to the end of the bookmarks");
    assert_eq!(sync.poll(), None, "the merge itself does not come back as a change");
}

#[test]
fn changes_made_in_explorer_reach_flit() {
    let mut sync = running(Fake::with(&[r"C:\Work", r"D:\Photos"]));
    sync.quick.explorer(&[r"D:\Photos", r"E:\Music"]);
    let change = sync.poll().unwrap();
    assert_eq!(change, Change { added: vec![p(r"E:\Music")], removed: vec![p(r"C:\Work")] });
    let flit = [l(r"C:\Work"), l(r"C:\Notes.txt"), l(r"D:\Photos")];
    assert_eq!(apply_incoming(&flit, &change).unwrap(), [l(r"C:\Notes.txt"), l(r"D:\Photos"), l(r"E:\Music")]);
    assert_eq!(sync.poll(), None, "nothing new without a new change");
}

#[test]
fn a_change_flit_made_itself_does_not_come_back() {
    let mut sync = running(Fake::with(&[r"C:\Work"]));
    sync.flit_changed(&[l(r"C:\Work")], &[l(r"C:\Work"), l(r"D:\Photos")]);
    assert_eq!(sync.poll(), None, "Flit's own pin is not reported as an Explorer change");
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music"]);
    let change = sync.poll().unwrap();
    let flit = [l(r"C:\Work"), l(r"D:\Photos")];
    let after = apply_incoming(&flit, &change).unwrap();
    sync.quick.calls.clear();
    sync.flit_changed(&flit, &after);
    assert!(sync.quick().changes().is_empty(), "applying Explorer's change in Flit does not pin it again");
    assert_eq!(apply_incoming(&after, &change), None, "applying the same change twice changes nothing");
}

#[test]
fn a_folder_on_a_missing_drive_is_not_unpinned_either_way() {
    let mut quick = Fake::with(&[r"C:\Work", r"Z:\Projects"]);
    quick.missing = vec![p(r"Z:\Projects")];
    let mut sync = running(quick);
    sync.flit_changed(&[l(r"C:\Work"), l(r"Z:\Projects")], &[l(r"C:\Work")]);
    assert!(sync.quick().changes().is_empty(), "a bookmark dropped because its drive is gone stays in Quick access");
    sync.quick.explorer(&[r"C:\Work"]);
    assert_eq!(sync.poll(), None, "a folder Quick access stops listing while its drive is gone stays in Flit");
    sync.quick.missing.clear();
    sync.quick.explorer(&[r"C:\Work", r"Z:\Projects"]);
    assert_eq!(sync.poll(), None, "when the drive comes back it is not a new pin");
}

#[test]
fn files_and_other_places_are_not_pinned_to_quick_access() {
    let mut quick = Fake::with(&[]);
    quick.missing = vec![p(r"C:\Notes.txt")];
    let mut sync = running(quick);
    sync.flit_changed(&[], &[l(r"C:\Notes.txt"), l(r"\\wsl.localhost\Ubuntu\home"), Location::recycle_bin()]);
    assert!(sync.quick().changes().is_empty());
}

fn four() -> QuickSync<Fake> {
    running(Fake::with(&[r"C:\Work", r"D:\Photos", r"E:\Music", r"F:\Docs"]))
}

#[test]
fn quick_access_reset_to_its_defaults_does_not_wipe_flit_bookmarks() {
    let mut sync = four();
    sync.quick.explorer(&[r"C:\Users\me\Desktop", r"C:\Users\me\Downloads"]);
    assert_eq!(sync.poll(), None, "a reset is remembered, nothing is removed from Flit");
    sync.quick.explorer(&[r"C:\Users\me\Desktop"]);
    assert_eq!(sync.poll().unwrap().removed, [p(r"C:\Users\me\Downloads")], "after the reset single changes work again");
}

#[test]
fn an_empty_list_or_several_folders_lost_at_once_remove_nothing() {
    let mut sync = running(Fake::with(&[r"C:\Work", r"D:\Photos"]));
    sync.quick.explorer(&[]);
    assert_eq!(sync.poll(), None, "an empty Quick access removes nothing");
    let mut sync = four();
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos"]);
    assert_eq!(sync.poll(), None, "two folders gone at once remove nothing");
    let mut sync = four();
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music"]);
    assert_eq!(sync.poll().unwrap().removed, [p(r"F:\Docs")], "one folder unpinned in Explorer is still removed");
}

#[test]
fn a_partial_read_adds_but_never_removes() {
    let mut sync = four();
    sync.quick.partial = true;
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music", r"G:\New"]);
    sync.quick.pinned.retain(|x| !x.same(&p(r"F:\Docs")));
    let change = sync.poll().unwrap();
    assert_eq!(change.added, [p(r"G:\New")]);
    assert!(change.removed.is_empty());
    sync.quick.partial = false;
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music", r"F:\Docs", r"G:\New"]);
    assert_eq!(sync.poll(), None, "a folder missing only from the partial read is not new later");
}

#[test]
fn a_vanished_store_file_or_failed_read_removes_nothing() {
    let mut sync = four();
    sync.quick.no_stamp = true;
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music"]);
    assert_eq!(sync.poll(), None, "without the store file the list is only remembered");
    let mut sync = four();
    sync.quick.broken = true;
    sync.quick.explorer(&[]);
    assert_eq!(sync.poll(), None);
}

#[test]
fn a_failed_first_read_makes_the_next_read_the_baseline() {
    let mut quick = Fake::with(&[r"C:\Work", r"D:\Photos"]);
    quick.broken = true;
    let mut sync = running(quick);
    sync.quick.broken = false;
    sync.quick.stamp += 1;
    assert_eq!(sync.poll(), None, "Quick access folders are not all added as new");
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music"]);
    assert_eq!(sync.poll().unwrap().added, [p(r"E:\Music")]);
}

#[test]
fn a_stale_read_right_after_flit_pins_or_unpins_does_not_undo_it() {
    let mut sync = four();
    sync.flit_changed(&[l(r"C:\Work")], &[l(r"C:\Work"), l(r"G:\New")]);
    sync.flit_changed(&[l(r"C:\Work"), l(r"F:\Docs")], &[l(r"C:\Work")]);
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music", r"F:\Docs"]);
    assert_eq!(sync.poll(), None, "the old list seen right after Flit's own change is not an Explorer change");
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music", r"G:\New"]);
    for _ in 0..=HOLD_POLLS {
        sync.quick.stamp += 1;
        assert_eq!(sync.poll(), None, "once settled, the list Flit made is not reported back");
    }
    sync.quick.explorer(&[r"C:\Work", r"D:\Photos", r"E:\Music"]);
    assert_eq!(sync.poll().unwrap().removed, [p(r"G:\New")], "after the hold an Explorer unpin of the same folder comes through");
}

#[test]
fn letter_case_beyond_ascii_does_not_make_duplicates() {
    let mut sync = QuickSync::new(Fake::with(&[r"C:\Проекты"]));
    let flit = [l(r"C:\проекты")];
    let change = sync.enable(&flit);
    assert!(change.added.is_empty(), "{change:?}");
    assert!(sync.quick().changes().is_empty());
}
