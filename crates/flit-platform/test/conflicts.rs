use super::*;
use flit_core::provider::Resolve;
use flit_core::{FsError, Meta};
use std::collections::HashMap;

#[derive(Default)]
struct Files {
    items: HashMap<FsPath, Meta>,
}

impl Fs for Files {
    fn read_dir(&self, dir: &FsPath, listing: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        for (path, &meta) in &self.items {
            if path.parent().as_ref() == Some(dir) {
                listing.push(path.file_name(), meta);
            }
        }
        listing.batch_done();
        Ok(())
    }

    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }

    fn operation_meta(&self, path: &FsPath) -> Option<Meta> {
        self.items.get(path).copied()
    }
}

fn path(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn one_review_lists_every_existing_name_and_answers_late_conflicts_separately() {
    let folder = Meta { created: 1, attrs: attr::DIR, ..Meta::default() };
    let source = Meta { size: 17, ..Meta::default() };
    let existing = Meta { size: 25, ..Meta::default() };
    let fs = Files { items: [
        (path("/src/tree"), folder), (path("/out/tree"), folder),
        (path("/src/tree/a"), source), (path("/out/tree/a"), existing),
        (path("/src/b"), source), (path("/out/b"), existing),
        (path("/src/new"), source),
    ].into_iter().collect() };
    let op = FileOp::Copy { items: vec![path("/src/tree"), path("/src/new"), path("/src/b")], dest: path("/out") };
    let ctl = OpCtl::default();
    let found = collect(&fs, &op, &ctl);
    assert_eq!(found.iter().map(|c| &c.target).collect::<Vec<_>>(), [&path("/out/tree"), &path("/out/tree/a"), &path("/out/b")]);
    assert!(ctl.preflight(found.clone(), |batch| {
        assert_eq!(batch, found);
        ctl.answer_conflicts(vec![Resolve::Replace, Resolve::Skip, Resolve::KeepBoth]);
    }));
    assert_eq!(ctl.resolve(&found[1], || panic!("an approved conflict prompted again")), Some(Resolve::Skip));
    let touched_folder = Conflict { to: Meta { modified: 123, ..folder }, ..found[0].clone() };
    assert_eq!(ctl.resolve(&touched_folder, || panic!("changing a directory's contents changed its merge decision")), Some(Resolve::Replace));
    let replaced_folder = Conflict { to: Meta { created: 456, ..folder }, ..found[0].clone() };
    assert_eq!(ctl.resolve(&replaced_folder, || ctl.answer(Resolve::Skip, false)), Some(Resolve::Skip));

    let changed = Conflict { to: Meta { size: 99, ..existing }, ..found[2].clone() };
    assert_eq!(ctl.resolve(&changed, || ctl.answer(Resolve::Skip, false)), Some(Resolve::Skip));
    let late = Conflict { source: path("/src/new"), target: path("/out/new"), from: source, to: existing };
    assert_eq!(ctl.resolve(&late, || ctl.answer(Resolve::KeepBoth, false)), Some(Resolve::KeepBoth));
}

#[test]
fn duplicating_a_file_or_using_a_previous_policy_does_not_open_review() {
    let item = path("/source/a");
    let fs = Files { items: [(item.clone(), Meta::default()), (path("/other/a"), Meta::default())].into_iter().collect() };
    let op = |dest| FileOp::Copy { items: vec![item.clone()], dest: path(dest) };
    assert!(collect(&fs, &op("/source"), &OpCtl::default()).is_empty());
    assert!(collect(&fs, &op("/other"), &OpCtl::new(Some(Resolve::Replace))).is_empty());
}

#[test]
fn a_review_above_the_row_limit_uses_individual_questions_with_apply_to_all() {
    let names: Vec<_> = (0..=5000).map(|n| format!("file-{n}")).collect();
    let source = path("/source");
    let target = path("/target");
    let items: Vec<_> = names.iter().map(|n| source.join(n.as_bytes())).collect();
    let fs = Files { items: names.iter().flat_map(|n| [
        (source.join(n.as_bytes()), Meta::default()), (target.join(n.as_bytes()), Meta::default()),
    ]).collect() };
    let op = |count| FileOp::Copy { items: items[..count].to_vec(), dest: target.clone() };
    let ctl = OpCtl::default();
    let within_limit = collect(&fs, &op(5000), &ctl);
    assert_eq!(within_limit.len(), 5000);
    assert!(collect(&fs, &op(5001), &ctl).is_empty());
    assert_eq!(ctl.resolve(&within_limit[0], || ctl.answer(Resolve::Skip, true)), Some(Resolve::Skip));
    assert_eq!(ctl.resolve(&within_limit[1], || panic!("apply to all must cover later conflicts")), Some(Resolve::Skip));
}

#[test]
fn properties_and_conflict_collection_use_foreground_lists() {
    struct Priorities {
        files: Files,
        background: std::sync::atomic::AtomicUsize,
    }
    impl Fs for Priorities {
        fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
            self.files.read_dir(dir, out)
        }
        fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
            Some(true)
        }
        fn operation_meta(&self, path: &FsPath) -> Option<Meta> {
            self.files.operation_meta(path)
        }
        fn read_background_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
            self.background.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.read_dir(dir, out)
        }
    }

    let dir = Meta { attrs: attr::DIR, ..Meta::default() };
    let file = Meta { size: 123, ..Meta::default() };
    let fs = Priorities {
        files: Files { items: [
            (path("/source/tree"), dir), (path("/target/tree"), dir),
            (path("/source/tree/file"), file), (path("/target/tree/file"), file),
        ].into_iter().collect() },
        background: Default::default(),
    };
    let op = FileOp::Copy { items: vec![path("/source/tree")], dest: path("/target") };
    assert_eq!(collect(&fs, &op, &OpCtl::default()).len(), 2);
    let totals = fs.measure(&[(path("/source/tree"), None)], &|| true, &mut |_| {});
    assert_eq!((totals.files, totals.size), (1, 123));
    assert_eq!(fs.background.load(std::sync::atomic::Ordering::Relaxed), 0);
    fs.dir_sizes(&path("/source"), &[(0, b"tree".to_vec())], &|| true, &|_, stats| assert_eq!(stats.size, 123));
    assert_eq!(fs.background.load(std::sync::atomic::Ordering::Relaxed), 1);
}
