use super::*;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn folders_pinned_to_quick_access_become_bookmarks_in_their_order() {
    let items = vec![(p(r"C:\Work"), true), (p(r"C:\Temp"), false), (p(r"\\nas\share\Docs"), true), (p(r"D:\Photos"), true)];
    assert_eq!(imported(items, &[]), [Location::from_local(p(r"C:\Work")), Location::from_local(p(r"\\nas\share\Docs")), Location::from_local(p(r"D:\Photos"))]);
}

#[test]
fn folders_already_among_places_and_repeats_are_not_imported_twice() {
    let known = [p(r"C:\Users\me\Desktop"), p(r"C:\Users\me\Downloads")];
    let items = vec![(p(r"C:\Users\me\Desktop"), true), (p(r"c:\users\me\downloads"), true), (p(r"C:\Work"), true), (p(r"c:\work"), true)];
    assert_eq!(imported(items, &known), [Location::from_local(p(r"C:\Work"))]);
}

#[test]
fn nothing_pinned_imports_nothing() {
    assert!(imported(Vec::new(), &[]).is_empty());
    assert!(imported(vec![(p(r"C:\Temp"), false)], &[]).is_empty(), "frequent folders that are not pinned stay out");
}
