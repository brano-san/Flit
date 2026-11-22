use super::*;
use crate::path::FsPath;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn report(op: FileOp, items: &[(&str, &str)]) -> OpReport {
    OpReport::new(op, items.iter().map(|&(a, b)| (p(a), p(b))).collect())
}

#[test]
fn replacing_a_destination_adds_its_restore_to_the_same_step() {
    let original = p("/target/a");
    let backup = p("/trash/files/a");
    for (op, first) in [
        (FileOp::Copy { items: vec![p("/source/a")], dest: p("/target") }, FileOp::Trash(vec![original.clone()])),
        (FileOp::Move { items: vec![p("/source/a")], dest: p("/target") }, FileOp::MoveTo(vec![(original.clone(), p("/source/a"))])),
    ] {
        let mut made = report(op, &[("/source/a", "/target/a")]);
        made.undo.replaced.push((original.clone(), Some(backup.clone())));
        let Some(Step::Checked(replay)) = inverse(&made) else { panic!("replacement must be reversed as one step") };
        assert_eq!(replay.steps, [first, FileOp::Restore(vec![(backup.clone(), original.clone())])]);
        made.undo.replaced[0].1 = None;
        assert!(matches!(inverse(&made), Some(Step::Refuse(_))));
    }
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
    let symlink = report(FileOp::Link { items: vec![p("/a/x")], dest: p("/b") }, &[("/a/x", "/b/Link to x")]);
    assert_eq!(inverse(&symlink), Some(Step::Run(FileOp::Delete(vec![p("/b/Link to x")]))), "a symbolic link is deleted, not trashed");
    let file = report(FileOp::NewFile { dir: p(r"C:\a"), name: b"New.txt".to_vec(), template: crate::provider::Template::Empty }, &[(r"C:\a\New.txt", r"C:\a\New.txt")]);
    assert_eq!(inverse(&file), Some(Step::Run(FileOp::Trash(vec![p(r"C:\a\New.txt")]))));
    let stream = report(FileOp::NewStream { item: p(r"C:\a\x"), name: b"s".to_vec() }, &[(r"C:\a\x", r"C:\a\x")]);
    assert_eq!(inverse(&stream), None, "streams are not in the Recycle Bin, so creating one is not undone");
}

#[test]
fn nothing_done_means_nothing_to_undo_and_permanent_deletes_refuse() {
    assert_eq!(inverse(&report(FileOp::Copy { items: vec![p(r"C:\a\x")], dest: p(r"C:\b") }, &[])), None);
    let gone = report(FileOp::Trash(vec![p(r"\\srv\s\x")]), &[(r"\\srv\s\x", r"\\srv\s\x")]);
    assert_eq!(inverse(&gone), Some(Step::Refuse(crate::i18n::S::ErrorsOpsNotRecycled.en())));
}

#[test]
fn an_unverified_system_transfer_has_a_distinct_reason_from_an_unbacked_replacement() {
    use crate::i18n::S;
    let mut moved = report(FileOp::Move { items: vec![p("/a")], dest: p("/target") }, &[("/a", "/target/a")]);
    moved.undo.unverified = true;
    assert_eq!(inverse(&moved), Some(Step::Refuse(S::ErrorsOpsUndoSystem.en())));
    assert!(moved.undo.replaced.is_empty());
    moved.undo.replaced.push((p("/target/a"), None));
    assert_eq!(inverse(&moved), Some(Step::Refuse(S::ErrorsOpsUndoBackup.en())));
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

#[test]
fn history_can_undo_an_older_independent_operation_and_keep_its_identity_for_redo() {
    let mut journal = Journal::default();
    let local = ProviderId::Local;
    let first = report(FileOp::Copy { items: vec![p("/source/a")], dest: p("/target") }, &[("/source/a", "/target/a")]);
    let second = report(FileOp::Copy { items: vec![p("/source/b")], dest: p("/target") }, &[("/source/b", "/target/b")]);
    journal.record(Origin::Fresh, local.clone(), &first);
    journal.record(Origin::Fresh, local.clone(), &second);
    let first_id = journal.entries(Origin::Undo)[0].id;
    let second_id = journal.entries(Origin::Undo)[1].id;
    assert!(journal.blockers(Origin::Undo, first_id).is_empty());
    assert_eq!(journal.begin(Origin::Undo, Some(first_id)), Some((local.clone(), Step::Run(FileOp::Trash(vec![p("/target/a")])))));
    journal.record(Origin::Undo, local.clone(), &report(FileOp::Trash(vec![p("/target/a")]), &[("/target/a", "/trash/files/a")]));
    assert_eq!(journal.entries(Origin::Undo)[0].id, second_id);
    assert_eq!(journal.entries(Origin::Redo)[0].id, first_id);
    let third = report(FileOp::Copy { items: vec![p("/source/c")], dest: p("/target") }, &[("/source/c", "/target/c")]);
    journal.record(Origin::Fresh, local, &third);
    assert!(journal.entries(Origin::Redo).is_empty());
}

#[test]
fn history_lists_transitive_dependencies_newest_first_and_failed_undo_remains_available() {
    let mut journal = Journal::default();
    let local = ProviderId::Local;
    for (a, b) in [("/a", "/b"), ("/b", "/c"), ("/other", "/independent"), ("/c", "/last")] {
        journal.record(Origin::Fresh, local.clone(), &report(FileOp::MoveTo(vec![(p(a), p(b))]), &[(a, b)]));
    }
    let ids: Vec<_> = journal.entries(Origin::Undo).iter().map(|entry| entry.id).collect();
    assert_eq!(journal.blockers(Origin::Undo, ids[0]), [ids[3], ids[1]]);
    assert!(journal.begin(Origin::Undo, Some(ids[3])).is_some());
    assert!(journal.running());
    journal.record(Origin::Undo, local, &OpReport::new(FileOp::MoveTo(Vec::new()), Vec::new()));
    assert!(!journal.running());
    assert_eq!(journal.entries(Origin::Undo).last().unwrap().id, ids[3]);
}
