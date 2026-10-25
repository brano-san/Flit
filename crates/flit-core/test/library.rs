use super::*;
use crate::sort::CaseMode;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn library_paths_are_recognised_by_extension() {
    assert!(is_library(&p(r"C:\Users\u\AppData\Roaming\Microsoft\Windows\Libraries\Documents.library-ms")));
    assert!(is_library(&p(r"\\srv\share\Team.LIBRARY-MS")));
    assert!(!is_library(&p(r"C:\x\notes.txt")));
    assert!(!is_library(&FsPath::parse("/home/u/a.library-ms").unwrap()));
    assert!(!is_library(&FsPath::home_page()));
    assert_eq!(stem(&p(r"C:\L\Documents.library-ms")), "Documents");
}

#[test]
fn merged_entries_remember_their_folder() {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    push(&mut b, b"a.txt", Meta::default(), &p(r"C:\one"));
    push(&mut b, b"a.txt", Meta::default(), &p(r"D:\two words"));
    b.push(b"stray", Meta::default());
    let l = b.listing;
    assert_eq!(l.name(0), b"a.txt");
    assert_eq!(folder(&l, 0), br"C:\one");
    assert_eq!(folder(&l, 1), br"D:\two words");
    assert_eq!(folder(&l, 2), b"");
    assert_eq!(b.keys.len(), 3);
}

#[test]
fn search_scope_matches_only_inside_the_folders() {
    assert_eq!(search_scope(&[]), None);
    let s = search_scope(&[p(r"C:\Users\John Doe\Documents"), p(r"D:\")]).unwrap();
    assert_eq!(s, r"path:regex:^(?:C\:\\Users\\John\x{20}Doe\\Documents|D\:)\\");
    assert!(!s.contains(' '), "one token for the query parser");
}
