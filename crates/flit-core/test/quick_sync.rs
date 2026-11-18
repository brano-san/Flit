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
    fn pinned(&mut self) -> Option<Vec<FsPath>> {
        self.calls.push("read".into());
        Some(self.pinned.clone())
    }

    fn stamp(&mut self) -> Option<(u64, u128)> {
        self.calls.push("stamp".into());
        Some((self.stamp, 0))
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
