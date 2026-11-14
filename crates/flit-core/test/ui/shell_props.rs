use super::*;
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::Persisted;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn app_in(path: &str, items: &[(&str, bool)]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn frame(app: &mut App) -> Vec<(TabId, u64, FsPath, Vec<Vec<u8>>)> {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    app.drain_effects().filter_map(|e| if let Effect::ShellProps { tab, gen, dir, names } = e { Some((tab, gen, dir, names)) } else { None }).collect()
}

fn cell(app: &App, name: &str, col: Column) -> String {
    let mut s = String::new();
    app.shell_prop_text(&mut s, name.as_bytes(), col);
    s
}

#[test]
fn a_shown_property_column_asks_for_the_visible_files_once_and_shows_what_arrives() {
    let mut app = app_in(r"C:\Media", &[("album", true), ("photo.jpg", false), ("song.mp3", false)]);
    assert!(frame(&mut app).is_empty(), "nothing is read while the columns are hidden");
    app.tab_mut().columns.toggle(Column::ImageSize);
    app.tab_mut().columns.toggle(Column::Artist);
    assert!(app.columns().contains(&Column::ImageSize) && app.columns().contains(&Column::Artist));
    let [(tab, gen, dir, names)] = frame(&mut app).try_into().unwrap();
    assert_eq!(dir, FsPath::parse(r"C:\Media").unwrap());
    assert_eq!(names, [b"photo.jpg".to_vec(), b"song.mp3".to_vec()], "folders have no such properties");
    assert!(frame(&mut app).is_empty(), "files already asked for are not asked again");
    let props = |v: [&str; 4]| v.map(str::to_owned).to_vec();
    app.on_shell_props(tab, gen + 1, vec![(b"photo.jpg".to_vec(), props(["9 x 9", "", "", ""]))]);
    assert_eq!(cell(&app, "photo.jpg", Column::ImageSize), "", "a reply for an older listing is dropped");
    app.on_shell_props(tab, gen, vec![(b"photo.jpg".to_vec(), props(["4000 x 3000", "", "", ""])), (b"song.mp3".to_vec(), props(["", "00:03:41", "Nina Simone", ""]))]);
    assert_eq!(cell(&app, "photo.jpg", Column::ImageSize), "4000 x 3000");
    assert_eq!(cell(&app, "song.mp3", Column::Duration), "00:03:41");
    assert_eq!(cell(&app, "song.mp3", Column::Artist), "Nina Simone");
    assert_eq!(cell(&app, "song.mp3", Column::Version), "");
}

#[test]
fn property_columns_read_nothing_inside_archives_the_recycle_bin_or_on_linux() {
    for path in [r"C:\Media\a.zip", "shell:RecycleBinFolder"] {
        let mut app = app_in(path, &[("photo.jpg", false)]);
        app.tab_mut().columns.toggle(Column::ImageSize);
        assert!(frame(&mut app).is_empty(), "{path}");
    }
    let mut linux = app_in(r"C:\Media", &[("photo.jpg", false)]);
    linux.caps.unix_paths = true;
    linux.tab_mut().columns.toggle(Column::ImageSize);
    assert!(frame(&mut linux).is_empty());
    assert!(!linux.column_kinds().contains(&Column::ImageSize));
}
