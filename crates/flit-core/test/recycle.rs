use super::*;
use crate::listing::attr;
use crate::sort::CaseMode;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn entries_carry_their_recycled_and_original_paths_after_the_name() {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    b.push_utf16(&wide("plain.txt"), Meta::default());
    push(&mut b, &wide("отчёт.docx"), Meta { size: 7, ..Default::default() }, &p(r"C:\$Recycle.Bin\S-1\$RAB12CD.docx"), &p(r"C:\Users\me\Документы"));
    push(&mut b, &wide("old"), Meta { attrs: attr::DIR, ..Default::default() }, &p(r"D:\$Recycle.Bin\S-1\$R000001"), &p(r"D:\"));
    b.push(b"after", Meta::default());
    let l = &b.listing;
    assert_eq!(l.name(1), "отчёт.docx".as_bytes());
    assert_eq!(l.ext(1), b"docx");
    assert_eq!(recycled(l, 1), Some(p(r"C:\$Recycle.Bin\S-1\$RAB12CD.docx")));
    assert_eq!(origin(l, 1), r"C:\Users\me\Документы".as_bytes());
    assert_eq!(original(l, 1), Some(p(r"C:\Users\me\Документы\отчёт.docx")));
    assert_eq!(original(l, 2), Some(p(r"D:\old")));
    assert_eq!(l.name(3), b"after", "an entry pushed after a tail keeps its own name");
    assert_eq!((recycled(l, 0), origin(l, 0), original(l, 0)), (None, &b""[..], None));
    assert_eq!((recycled(l, 3), original(l, 3)), (None, None));
}

#[test]
fn recycled_paths_are_recognised_by_their_bin_folder() {
    assert!(in_bin(&p(r"C:\$Recycle.Bin\S-1\$RAB12CD.txt")));
    assert!(in_bin(&p(r"d:\$RECYCLE.BIN\S-1\$R1\inner")));
    assert!(!in_bin(&p(r"C:\Users\me\Recycle.Bin")));
}
