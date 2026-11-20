use super::*;

fn location(path: &str) -> Location {
    Location::parse(path).unwrap()
}

fn app(folder: Location, base: TabId) -> App {
    App::new(Persisted::default(), vec![Route { folder, select: None }], base, false, 1.0, (1100, 720), Instant::now())
}

#[test]
fn bookmarks_clear_removes_only_flit_bookmarks() {
    let first = location(r"C:\Photos");
    let second = location(r"C:\Music");
    let mut source = app(first.clone(), 0);
    let mut other = app(second.clone(), TAB_STRIDE);
    let mut nav = source.nav_state();
    let mut pending = VecDeque::new();
    let mut quick = Vec::new();
    let mut desktop = Vec::new();
    let mut apply = |source: &mut App, nav: &mut NavState, other: &mut App| {
        for effect in source.drain_effects() {
            apply_bookmark_effect(effect, nav, std::iter::once(&mut *other), &mut pending, |old, new| quick.push((old.to_vec(), new.to_vec())), |add, remove| desktop.push((add.to_vec(), remove.to_vec())));
        }
    };

    source.pin_locations(vec![first.clone(), second.clone()]);
    apply(&mut source, &mut nav, &mut other);
    assert_eq!(nav.pinned, [first.clone(), second.clone()]);
    assert_eq!(other.nav_state().pinned, nav.pinned);

    assert!(source.run_action("bookmarks.clear", Instant::now()));
    apply(&mut source, &mut nav, &mut other);
    assert!(source.nav_state().pinned.is_empty() && other.nav_state().pinned.is_empty() && nav.pinned.is_empty());
    assert_eq!(quick, [(vec![], vec![first.clone(), second.clone()])], "clearing does not send Quick access removals");
    assert_eq!(desktop, [(vec![first, second], vec![])], "clearing does not send GTK or KDE removals");
}

#[test]
fn bookmarks_unpinning_a_folder_still_notifies_quick_access_and_the_desktop() {
    let first = location(r"C:\Photos");
    let second = location(r"C:\Music");
    let mut source = app(first.clone(), 0);
    source.pin_locations(vec![first.clone(), second.clone()]);
    let mut nav = source.nav_state();
    source.drain_effects().for_each(drop);
    assert!(source.run_action("folder.bookmark", Instant::now()));

    let mut quick = Vec::new();
    let mut desktop = Vec::new();
    for effect in source.drain_effects() {
        apply_bookmark_effect(effect, &mut nav, std::iter::empty(), &mut VecDeque::new(), |old, new| quick.push((old.to_vec(), new.to_vec())), |add, remove| desktop.push((add.to_vec(), remove.to_vec())));
    }
    assert_eq!(nav.pinned.as_slice(), std::slice::from_ref(&second));
    assert_eq!(desktop, [(vec![], vec![first.clone()])], "the desktop receives the removed folder");
    assert_eq!(quick, [(vec![first, second.clone()], vec![second])], "Quick access receives the corresponding pin change");
}

#[test]
fn bookmarks_pending_windows_receive_the_new_folders_and_files() {
    let folder = location(r"C:\Photos");
    let file = location(r"C:\Photos\note.txt");
    let mut source = app(folder.clone(), 0);
    let mut ready = app(folder.clone(), TAB_STRIDE);
    let mut pending = VecDeque::from([app(folder.clone(), 2 * TAB_STRIDE)]);
    let mut nav = source.nav_state();
    source.pin_entries(vec![folder.clone()], vec![file.clone()]);
    for effect in source.drain_effects() {
        apply_bookmark_effect(effect, &mut nav, std::iter::once(&mut ready), &mut pending, |_, _| {}, |_, _| {});
    }
    let waiting = pending.pop_front().unwrap().nav_state();
    assert_eq!(ready.nav_state().pinned, [folder.clone(), file.clone()]);
    assert_eq!(waiting.pinned, [folder, file.clone()]);
    assert_eq!(waiting.files, [file]);
    assert_eq!(waiting.files, nav.files);
}
