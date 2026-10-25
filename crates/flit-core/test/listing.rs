use super::*;

#[test]
fn compact_entry() {
    assert_eq!(std::mem::size_of::<Entry>(), 40);
}

#[test]
fn single_name_buffer() {
    let mut l = Listing::default();
    let file = Meta { size: 5, ..Default::default() };
    let dir = Meta { attrs: attr::DIR, ..Default::default() };
    l.push(b"a.txt", file);
    l.push_utf16(&"Папка.d".encode_utf16().collect::<Vec<_>>(), dir);
    l.push_utf16(&[b'x' as u16, 0xD800, b'.' as u16, b'r' as u16, b's' as u16], file);
    assert_eq!(l.name(0), b"a.txt");
    assert_eq!(l.ext(0), b"txt");
    assert_eq!(l.name(1), "Папка.d".as_bytes());
    assert_eq!(l.ext(1), b"", "directories have no extension");
    assert_eq!(l.ext(2), b"rs");
    assert_eq!(name::display(l.name(2)), "x\u{FFFD}.rs");
    assert_eq!(l.names.len(), 5 + "Папка.d".len() + 7);
}

#[test]
fn builder_batches_and_cancel() {
    let mut batches = 0;
    let mut cb = |b: &ListingBuilder<'_>| {
        batches += 1;
        b.listing.len() < 2
    };
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut cb);
    b.push(b"one", Meta::default());
    assert!(b.batch_done());
    b.push(b"two", Meta::default());
    assert!(!b.batch_done());
    assert_eq!(b.keys.len(), 2);
    drop(b);
    assert_eq!(batches, 2);
}
