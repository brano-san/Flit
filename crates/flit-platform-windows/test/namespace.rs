use super::*;
use flit_core::sort::CaseMode;
use std::cell::Cell;

#[derive(Clone)]
struct Fake {
    name: &'static str,
    kids: Vec<Fake>,
    size: u64,
}

thread_local! {
    static OPENED: Cell<usize> = const { Cell::new(0) };
}

fn dir(name: &'static str, kids: Vec<Fake>) -> Fake {
    Fake { name, kids, size: 0 }
}

fn file(name: &'static str, size: u64) -> Fake {
    Fake { name, kids: Vec::new(), size }
}

impl Node for Fake {
    fn name(&self) -> Vec<u8> {
        self.name.as_bytes().to_vec()
    }
    fn meta(&self) -> Meta {
        Meta { size: self.size, attrs: if self.size == 0 { attr::DIR } else { 0 }, ..Default::default() }
    }
    fn children(&self, each: &mut dyn FnMut(Self) -> bool) -> Result<(), FsError> {
        if self.name == "Locked" {
            return Err(FsError { kind: FsErrorKind::AccessDenied, code: 5 });
        }
        for k in &self.kids {
            OPENED.set(OPENED.get() + 1);
            if !each(k.clone()) {
                break;
            }
        }
        Ok(())
    }
}

fn phone() -> Fake {
    let photos: Vec<Fake> = (0..100).map(|k| file(Box::leak(format!("IMG_{k:04}.jpg").into_boxed_str()), 1000 + k)).collect();
    dir("This PC", vec![dir("Pixel 7", vec![dir("Internal shared storage", vec![dir("DCIM", vec![dir("Camera", photos)]), dir("Locked", vec![])])])])
}

#[test]
fn walk_matches_names_without_case_and_stops_at_the_hit() {
    OPENED.set(0);
    let camera = walk(phone(), &[b"pixel 7", b"Internal shared storage", b"dcim", b"Camera"]).unwrap();
    assert_eq!(camera.kids.len(), 100);
    assert_eq!(OPENED.get(), 4);
    assert_eq!(walk(phone(), &[b"Pixel 7", b"SD card"]).err(), Some(not_found()));
}

#[test]
fn listing_streams_in_batches_and_stops_when_cancelled() {
    let camera = walk(phone(), &[b"Pixel 7", b"Internal shared storage", b"DCIM", b"Camera"]).unwrap();
    let mut batches = 0;
    let mut on_batch = |_: &ListingBuilder<'_>| {
        batches += 1;
        true
    };
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch);
    list(&camera, &mut b).unwrap();
    assert_eq!(b.listing.len(), 100);
    drop(b);
    assert_eq!(batches, 4);
    OPENED.set(0);
    let mut stop = |_: &ListingBuilder<'_>| false;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut stop);
    list(&camera, &mut b).unwrap();
    assert_eq!((b.listing.len(), OPENED.get()), (BATCH, BATCH));
}

#[test]
fn enumeration_errors_reach_the_listing() {
    let storage = walk(phone(), &[b"Pixel 7", b"Internal shared storage"]).unwrap();
    let locked = walk(storage, &[b"Locked"]).unwrap();
    let mut on_batch = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch);
    assert_eq!(list(&locked, &mut b).err().map(|e| e.kind), Some(FsErrorKind::AccessDenied));
}

#[test]
fn network_root_resolves_through_the_real_shell() {
    let net = Item(item(&FsPath::network()).expect("the Network folder exists on every Windows"));
    assert!(net.meta().attrs & attr::DIR != 0);
    assert_eq!(item(&FsPath::device(b"No Such Phone 123").unwrap()).err(), Some(not_found()));
}
