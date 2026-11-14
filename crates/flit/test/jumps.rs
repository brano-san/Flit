use super::*;
use crossbeam_channel::Receiver;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn sent(rx: &Receiver<Lists>) -> Vec<Lists> {
    rx.try_iter().collect()
}

#[test]
fn the_jump_list_is_written_again_only_when_bookmarks_or_frequent_folders_change() {
    let (tx, rx) = unbounded();
    let mut j = JumpList::with(Some(tx));
    let mut nav = NavState { pinned: vec![loc(r"C:\Work")], ..NavState::default() };
    let mut prefs = NavPrefs::default();
    prefs.count_visit(&loc(r"C:\Temp"));
    j.show(lists(&nav, &prefs));
    j.show(lists(&nav, &prefs));
    assert_eq!(sent(&rx), [(vec![loc(r"C:\Work")], vec![loc(r"C:\Temp")])], "the same lists are not written twice");
    prefs.count_visit(&loc(r"D:\Music"));
    j.show(lists(&nav, &prefs));
    nav.pinned.push(loc(r"D:\Photos"));
    j.show(lists(&nav, &prefs));
    let both = sent(&rx);
    assert_eq!(both.len(), 2);
    assert_eq!(both[1].0, [loc(r"C:\Work"), loc(r"D:\Photos")]);
}

#[test]
fn a_jump_list_that_is_off_writes_nothing_and_stays_quiet() {
    let mut j = JumpList::with(None);
    j.show((vec![loc(r"C:\Work")], Vec::new()));
    j.show((Vec::new(), Vec::new()));
    assert!(j.tx.is_none());
}
