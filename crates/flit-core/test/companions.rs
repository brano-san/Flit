use super::*;
use crate::listing::{ListingBuilder, Meta};
use crate::sort::CaseMode;

#[test]
fn exact_xmp_and_appledouble_bind_to_the_file_and_follow_its_new_name() {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut listing = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    for name in [b"X.jpg".as_slice(), b"._X.jpg", b"x.jpg.xmp", b"x.aae", b"orphan.xmp"] {
        listing.push(name, Meta::default());
    }
    let linked = associated(&listing.listing, false);
    let mapped: Vec<_> = linked.iter().map(|side| (side.owner, side.sidecar, side.kind.beside(b"new.png"))).collect();
    assert_eq!(mapped, [(0, 1, b"._new.png".to_vec()), (0, 2, b"new.png.xmp".to_vec()), (0, 3, b"new.aae".to_vec())]);
}

#[test]
fn an_ambiguous_stem_and_an_unpaired_appledouble_remain_visible() {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut listing = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    for name in [b"x.jpg".as_slice(), b"x.png", b"x.xmp", b"._orphan", b"x.png.xmp"] {
        listing.push(name, Meta::default());
    }
    assert_eq!(hidden(&listing.listing, true).into_iter().collect::<Vec<_>>(), vec![4]);
}
