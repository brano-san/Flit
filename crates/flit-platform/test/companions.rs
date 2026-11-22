use super::*;
use flit_core::listing::{FsError, Meta};
use flit_core::provider::Resolve;

struct Directory;

impl Fs for Directory {
    fn read_dir(&self, _: &FsPath, b: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        for name in [b"photo.jpg".as_slice(), b"photo.jpg.xmp", b"._photo.jpg", b"unpaired.xmp"] {
            b.push(name, Meta::default());
        }
        Ok(())
    }

    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
}

#[test]
fn copying_and_moving_a_primary_include_explicitly_selected_companions_once() {
    let dir = FsPath::parse("/source").unwrap();
    let dest = FsPath::parse("/destination").unwrap();
    let primary = dir.join(b"photo.jpg");
    let xmp = dir.join(b"photo.jpg.xmp");
    let apple = dir.join(b"._photo.jpg");
    let orphan = dir.join(b"unpaired.xmp");
    for moving in [false, true] {
        let job = Job::detached(Resolve::Skip);
        let op = |items| if moving { FileOp::Move { items, dest: dest.clone() } } else { FileOp::Copy { items, dest: dest.clone() } };
        assert_eq!(job.prepare_companions(&Directory, op(vec![xmp.clone(), primary.clone(), orphan.clone()])).unwrap(), op(vec![primary.clone(), orphan.clone()]));
        assert_eq!(job.companion_targets(&primary, &dest.join(b"renamed.jpg")), [(xmp.clone(), dest.join(b"renamed.jpg.xmp")), (apple.clone(), dest.join(b"._renamed.jpg")),]);
        assert!(job.companion_targets(&orphan, &dest.join(b"unpaired.xmp")).is_empty());
    }
}

#[test]
fn cancellation_is_deferred_until_a_transfer_leaves_its_group() {
    let dir = FsPath::parse("/source").unwrap();
    let primary = dir.join(b"photo.jpg");
    let job = Job::detached(Resolve::Skip);
    job.prepare_companions(&Directory, FileOp::Copy { items: vec![primary.clone()], dest: dir }).unwrap();
    {
        let _group = job.defer_companion_cancellation(&primary).unwrap();
        job.ctl().cancel();
        assert!(!job.cancelled());
        assert!(job.defer_companion_cancellation(&primary).is_none());
        assert!(!job.cancelled());
    }
    assert!(job.cancelled());
}
