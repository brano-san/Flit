use super::*;
use crate::path::FsPath;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn report(op: FileOp, items: &[(&str, &str)]) -> OpReport {
    OpReport { op, items: items.iter().map(|&(a, b)| (p(a), p(b))).collect() }
}

#[test]
fn every_operation_has_its_inverse() {
    let copy = report(FileOp::Copy { items: vec![p(r"C:\a\x")], dest: p(r"C:\b") }, &[(r"C:\a\x", r"C:\b\x - Copy")]);
    assert_eq!(inverse(&copy), Some(Step::Run(FileOp::Trash(vec![p(r"C:\b\x - Copy")]))));
    let moved = report(FileOp::Move { items: vec![p(r"C:\a\x")], dest: p(r"C:\b") }, &[(r"C:\a\x", r"C:\b\x (2)")]);
    assert_eq!(inverse(&moved), Some(Step::Run(FileOp::MoveTo(vec![(p(r"C:\b\x (2)"), p(r"C:\a\x"))]))));
    let renamed = report(FileOp::Rename { item: p(r"C:\a\x.txt"), name: b"y.txt".to_vec() }, &[(r"C:\a\x.txt", r"C:\a\y.txt")]);
    assert_eq!(inverse(&renamed), Some(Step::Run(FileOp::Rename { item: p(r"C:\a\y.txt"), name: b"x.txt".to_vec() })));
    let folder = report(FileOp::NewFolder { dir: p(r"C:\a"), name: b"New folder".to_vec() }, &[(r"C:\a\New folder", r"C:\a\New folder")]);
    assert_eq!(inverse(&folder), Some(Step::Run(FileOp::Trash(vec![p(r"C:\a\New folder")]))));
    let trash = report(FileOp::Trash(vec![p(r"C:\a\x"), p(r"C:\a\y")]), &[(r"C:\a\x", r"C:\$Recycle.Bin\S\$R1"), (r"C:\a\y", r"C:\a\y")]);
    assert_eq!(inverse(&trash), Some(Step::Run(FileOp::Restore(vec![(p(r"C:\$Recycle.Bin\S\$R1"), p(r"C:\a\x"))]))));
    let restore = report(FileOp::Restore(vec![(p(r"C:\$Recycle.Bin\S\$R1"), p(r"C:\a\x"))]), &[(r"C:\$Recycle.Bin\S\$R1", r"C:\a\x")]);
    assert_eq!(inverse(&restore), Some(Step::Run(FileOp::Trash(vec![p(r"C:\a\x")]))));
    let link = report(FileOp::Link { items: vec![p(r"C:\a\x")], dest: p(r"C:\b") }, &[(r"C:\a\x", r"C:\b\x - Shortcut.lnk")]);
    assert_eq!(inverse(&link), Some(Step::Run(FileOp::Trash(vec![p(r"C:\b\x - Shortcut.lnk")]))));
    let file = report(FileOp::NewFile { dir: p(r"C:\a"), name: b"New.txt".to_vec(), template: crate::provider::Template::Empty }, &[(r"C:\a\New.txt", r"C:\a\New.txt")]);
    assert_eq!(inverse(&file), Some(Step::Run(FileOp::Trash(vec![p(r"C:\a\New.txt")]))));
    let stream = report(FileOp::NewStream { item: p(r"C:\a\x"), name: b"s".to_vec() }, &[(r"C:\a\x", r"C:\a\x")]);
    assert_eq!(inverse(&stream), None, "streams are not in the Recycle Bin, so creating one is not undone");
}

#[test]
fn nothing_done_means_nothing_to_undo_and_permanent_deletes_refuse() {
    assert_eq!(inverse(&report(FileOp::Copy { items: vec![p(r"C:\a\x")], dest: p(r"C:\b") }, &[])), None);
    let gone = report(FileOp::Trash(vec![p(r"\\srv\s\x")]), &[(r"\\srv\s\x", r"\\srv\s\x")]);
    assert_eq!(inverse(&gone), Some(Step::Refuse(NOT_RECYCLED)));
}

#[test]
fn journal_moves_steps_between_undo_and_redo() {
    let mut j = Journal::default();
    let local = ProviderId::Local;
    let copy = report(FileOp::Copy { items: vec![p(r"C:\a\x")], dest: p(r"C:\b") }, &[(r"C:\a\x", r"C:\b\x")]);
    j.record(Origin::Fresh, local.clone(), &copy);
    assert_eq!(j.depth(), (1, 0));
    assert_eq!(j.take(Origin::Undo), Some((local.clone(), Step::Run(FileOp::Trash(vec![p(r"C:\b\x")])))));
    let trashed = report(FileOp::Trash(vec![p(r"C:\b\x")]), &[(r"C:\b\x", r"C:\$Recycle.Bin\S\$R2")]);
    j.record(Origin::Undo, local.clone(), &trashed);
    assert_eq!(j.depth(), (0, 1));
    assert_eq!(j.take(Origin::Redo), Some((local.clone(), Step::Run(FileOp::Restore(vec![(p(r"C:\$Recycle.Bin\S\$R2"), p(r"C:\b\x"))])))));
    j.record(Origin::Redo, local.clone(), &report(FileOp::Restore(vec![]), &[(r"C:\$Recycle.Bin\S\$R2", r"C:\b\x")]));
    assert_eq!(j.depth(), (1, 0));
    j.record(Origin::Undo, local.clone(), &trashed);
    j.record(Origin::Fresh, local.clone(), &copy);
    assert_eq!(j.depth(), (2, 0), "a fresh operation clears redo");
    assert_eq!(j.take(Origin::Fresh), None);
    for _ in 0..DEPTH + 10 {
        j.record(Origin::Fresh, local.clone(), &copy);
    }
    assert_eq!(j.depth(), (DEPTH, 0));
}

#[test]
fn linux_trash_is_undone_by_a_restore_in_the_same_distro() {
    let mut j = Journal::default();
    let wsl = ProviderId::Wsl("Ubuntu".into());
    let trashed = report(FileOp::Trash(vec![p("/home/u/a:b")]), &[("/home/u/a:b", "/home/u/.local/share/Trash/files/a:b")]);
    j.record(Origin::Fresh, wsl.clone(), &trashed);
    assert_eq!(j.take(Origin::Undo), Some((wsl, Step::Run(FileOp::Restore(vec![(p("/home/u/.local/share/Trash/files/a:b"), p("/home/u/a:b"))])))));
}
