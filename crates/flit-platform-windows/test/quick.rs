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

type Item = Result<Option<(Option<FsPath>, bool)>, &'static str>;

fn listing(items: Vec<Item>) -> Result<Read, &'static str> {
    let mut it = items.into_iter();
    gather(|| it.next().unwrap_or(Ok(None)))
}

#[test]
fn an_error_in_the_middle_of_reading_quick_access_is_not_a_short_list() {
    let read = listing(vec![Ok(Some((Some(p(r"C:\Work")), true))), Err("enumeration failed")]);
    assert!(read.is_err());
    let read = listing(vec![Ok(Some((Some(p(r"C:\Work")), true))), Ok(Some((Some(p(r"C:\Temp")), false))), Ok(None)]).unwrap();
    assert!(read.complete);
    assert_eq!(read.items.len(), 2);
}

#[test]
fn the_read_limit_counts_pinned_folders_and_marks_the_list_partial() {
    let mut items: Vec<_> = (0..200).map(|i| Ok(Some((Some(p(&format!(r"C:\Frequent{i}"))), false)))).collect();
    items.push(Ok(Some((Some(p(r"C:\Work")), true))));
    let read = listing(items).unwrap();
    assert!(read.complete, "frequent folders do not use up the limit");
    assert!(read.items.iter().any(|(path, pinned)| *pinned && path.same(&p(r"C:\Work"))));
    let pinned: Vec<_> = (0..100).map(|i| Ok(Some((Some(p(&format!(r"C:\Pinned{i}"))), true)))).collect();
    let read = listing(pinned).unwrap();
    assert!(!read.complete, "a list cut at the limit is partial");
}
