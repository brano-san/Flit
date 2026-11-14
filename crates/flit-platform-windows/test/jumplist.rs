use super::*;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn titles(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.title.as_str()).collect()
}

#[test]
fn bookmarks_come_first_and_frequent_folders_fill_the_remaining_slots() {
    let pinned = [loc(r"C:\Work"), loc(r"D:\Photos")];
    let frequent = [loc(r"C:\Temp"), loc(r"C:\Work"), loc(r"E:\Music"), loc(r"F:\Extra")];
    let (p, f) = plan(&pinned, &frequent, &[], 4);
    assert_eq!(titles(&p), ["Work", "Photos"]);
    assert_eq!(titles(&f), ["Temp", "Music"], "a bookmark is not repeated among frequent folders and the list stops at the slot count");
    assert_eq!(p[0].args, r#""C:\Work""#);
}

#[test]
fn folders_the_user_removed_from_the_jump_list_stay_out() {
    let frequent = [loc(r"C:\Temp"), loc(r"C:\Work")];
    let (_, f) = plan(&[], &frequent, &[r#""c:\temp""#.to_owned()], 10);
    assert_eq!(titles(&f), ["Work"]);
}

#[test]
fn places_that_are_not_folders_on_disk_are_left_out_and_roots_keep_their_path() {
    let pinned = [Location::home_page(), Location::computer(), Location::recycle_bin(), loc(r"C:\"), loc(r"\\nas\share\docs")];
    let (p, _) = plan(&pinned, &[], &[], 10);
    assert_eq!(titles(&p), [r"C:\", "docs"]);
    assert_eq!(p[0].args, r#""C:\\""#, "a trailing backslash is doubled so it does not escape the closing quote");
}

#[test]
fn nothing_is_listed_without_slots_or_places() {
    assert_eq!(plan(&[loc(r"C:\Work")], &[loc(r"C:\Temp")], &[], 0), (Vec::new(), Vec::new()));
    assert_eq!(plan(&[], &[], &[], 10), (Vec::new(), Vec::new()));
}
