use super::*;

#[test]
fn linux_names_roundtrip_verbatim() {
    let mut b = EntryBatch::default();
    let names: [&[u8]; 5] = [b"plain", b"\xFF\xFEnot-utf8", "отчёт 🎉".as_bytes(), br"back\slash:colon?", b"\x01ctl"];
    for (i, n) in names.iter().enumerate() {
        b.push(n, Meta { size: i as u64, ..Default::default() });
    }
    let wire = postcard::to_stdvec(&b).unwrap();
    let back: EntryBatch = postcard::from_bytes(&wire).unwrap();
    let mut l = Listing::default();
    back.append_to(&mut l, false).unwrap();
    for (i, n) in names.iter().enumerate() {
        assert_eq!(l.name(i as u32), *n);
        assert_eq!(l.entries[i].size, i as u64);
    }
}

#[test]
fn hostile_batches_rejected() {
    for evil in [&b".."[..], b".", b"a/b", b"", b"nul\0"] {
        let mut b = EntryBatch::default();
        b.push(b"ok", Meta::default());
        b.push(evil, Meta::default());
        assert!(b.entries(false).is_err(), "{evil:?}");
        assert!(b.entries(true).is_err(), "{evil:?}");
    }
    let mut b = EntryBatch::default();
    b.push(b"abc", Meta::default());
    b.entries[0].name_len = 99;
    assert!(b.entries(false).is_err());
    b.entries[0].name_len = u32::MAX;
    b.entries.push(WireEntry { name_len: 4, meta: Meta::default() });
    assert!(b.entries(false).is_err(), "length sum must not wrap");
}

#[test]
fn windows_names_reject_backslash_and_colon() {
    for evil in [&br"..\..\Windows"[..], br"a\b", b"file.txt:stream", b"C:"] {
        let mut b = EntryBatch::default();
        b.push(evil, Meta::default());
        assert!(b.entries(false).is_ok(), "{evil:?}");
        assert!(b.entries(true).is_err(), "{evil:?}");
    }
}

#[test]
fn internal_attribute_bits_rejected() {
    for bit in [attr::DELETED, attr::SIZE_KNOWN, attr::VOLUME, 1 << 20] {
        let mut b = EntryBatch::default();
        b.push(b"x", Meta { attrs: attr::DIR | bit, ..Default::default() });
        assert!(b.entries(false).is_err(), "{bit:#x}");
    }
    let mut b = EntryBatch::default();
    b.push(b"x", Meta { attrs: WIRE_ATTRS, ..Default::default() });
    assert!(b.entries(true).is_ok());
}
