use super::*;

#[test]
fn oversized_archive_sizes_do_not_overflow_addition_progress() {
    let job = Job::detached(Resolve::Replace);
    let source = Source { path: PathBuf::new(), from: FsPath::parse("/new.txt").unwrap(), name: "new.txt".into(), dir: false, size: 1, modified: 0 };
    add_progress(&job, &[source], u64::MAX, u64::MAX);
    let progress = job.progress();
    assert_eq!(progress.stage, Stage::Running);
    assert_eq!((progress.total_items, progress.total_bytes), (u64::MAX, u64::MAX));
}
