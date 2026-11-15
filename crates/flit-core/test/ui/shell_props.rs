use super::*;
use crate::listing::{attr, ChangeKind, ColumnValues, DirChange, DirSnapshot, Listing, Meta};
use crate::provider::ProviderEvent;
use crate::sort::{self, CaseMode, SortField, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::Persisted;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Ask = (TabId, u64, u64, FsPath, Vec<Vec<u8>>);

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

fn frame_at(app: &mut App, now: Instant) -> Vec<Ask> {
    app.frame(&mut Gfx::new(Mono, 1024), now);
    let mut asks = Vec::new();
    for e in app.drain_effects().collect::<Vec<_>>() {
        match e {
            Effect::ShellProps { tab, gen, ask, dir, names } => asks.push((tab, gen, ask, dir, names)),
            Effect::Resort { tab, gen, view, snap } => {
                let mut order = Vec::new();
                sort::sort(&snap.listing, &snap.keys, view, &mut order);
                app.on_resorted(tab, gen, view, snap.listing.len(), order);
            }
            _ => {}
        }
    }
    asks
}

fn frame(app: &mut App) -> Vec<Ask> {
    frame_at(app, Instant::now())
}

fn cell(app: &App, name: &str, col: Column) -> String {
    let mut s = String::new();
    app.shell_prop_text(&mut s, name.as_bytes(), col);
    s
}

fn texts(v: [&str; 4]) -> ColumnValues {
    let mut text: Vec<String> = v.map(str::to_owned).to_vec();
    text.resize(crate::listing::SHELL_PROPS.len(), String::new());
    ColumnValues { text, ..Default::default() }
}

fn seconds_of(name: &[u8]) -> Option<u64> {
    let n = std::str::from_utf8(name).ok()?;
    let k: u64 = n.strip_prefix("song ")?.strip_suffix(".mp3")?.parse().ok()?;
    Some(k * 7919 % 3600)
}

fn reader(name: &[u8]) -> ColumnValues {
    match seconds_of(name) {
        Some(s) => ColumnValues { duration: Some(s * 10_000_000), ..texts(["", &format!("{}:{:02}", s / 60, s % 60), "", ""]) },
        None => texts(["", "", "", ""]),
    }
}

fn reply(app: &mut App, asks: Vec<Ask>) -> usize {
    let mut n = 0;
    for (tab, gen, ask, _, names) in asks {
        n += names.len();
        app.on_shell_props(tab, gen, ask, names.into_iter().map(|name| {
            let v = reader(&name);
            (name, v)
        }).collect());
    }
    n
}

fn media_app() -> App {
    let mut items = vec![("Playlists".to_owned(), true)];
    items.extend((0..300).map(|k| (format!("song {k:03}.mp3"), false)));
    items.extend((0..100).map(|k| (format!("notes {k:03}.txt"), false)));
    let refs: Vec<(&str, bool)> = items.iter().map(|(n, d)| (n.as_str(), *d)).collect();
    let mut app = app_in(r"C:\Media", &refs);
    frame(&mut app);
    app
}

fn shown_names(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn pick(app: &mut App, name: &str) {
    let v = app.tab_mut().view.as_mut().unwrap();
    let i = v.find(name.as_bytes()).unwrap();
    v.clear_selection();
    v.set_selected(i, true);
    v.focus = Some(i);
    v.anchor = Some(i);
}

fn picked(app: &App) -> (Vec<String>, Option<String>) {
    let v = app.tab().view.as_ref().unwrap();
    let name = |i: u32| String::from_utf8_lossy(v.listing().name(i)).into_owned();
    (v.selected().map(name).collect(), v.focus.map(name))
}

#[test]
fn a_shown_property_column_asks_for_the_visible_files_once_and_shows_what_arrives() {
    let mut app = app_in(r"C:\Media", &[("album", true), ("photo.jpg", false), ("song.mp3", false)]);
    assert!(frame(&mut app).is_empty(), "nothing is read while the columns are hidden");
    app.tab_mut().columns.toggle(Column::ImageSize);
    app.tab_mut().columns.toggle(Column::Artist);
    assert!(app.columns().contains(&Column::ImageSize) && app.columns().contains(&Column::Artist));
    let [(tab, gen, ask, dir, names)] = frame(&mut app).try_into().unwrap();
    assert_eq!(dir, FsPath::parse(r"C:\Media").unwrap());
    assert_eq!(names, [b"photo.jpg".to_vec(), b"song.mp3".to_vec()], "folders have no such properties");
    assert!(frame(&mut app).is_empty(), "files already asked for are not asked again");
    app.on_shell_props(tab, gen + 1, ask, vec![(b"photo.jpg".to_vec(), texts(["9 x 9", "", "", ""]))]);
    assert_eq!(cell(&app, "photo.jpg", Column::ImageSize), "", "a reply for an older listing is dropped");
    app.on_shell_props(tab, gen, ask, vec![(b"photo.jpg".to_vec(), texts(["4000 x 3000", "", "", ""])), (b"song.mp3".to_vec(), texts(["", "00:03:41", "Nina Simone", ""]))]);
    assert_eq!(cell(&app, "photo.jpg", Column::ImageSize), "4000 x 3000");
    assert_eq!(cell(&app, "song.mp3", Column::Duration), "00:03:41");
    assert_eq!(cell(&app, "song.mp3", Column::Artist), "Nina Simone");
    assert_eq!(cell(&app, "song.mp3", Column::Version), "");
}

#[test]
fn the_six_new_columns_show_what_the_shell_reports_in_their_own_cells() {
    let mut app = app_in(r"C:\Media", &[("photo.jpg", false)]);
    frame(&mut app);
    let extra = [Column::DateTaken, Column::CameraModel, Column::Album, Column::Title, Column::Authors, Column::Company];
    for c in extra {
        assert!(app.column_kinds().contains(&c), "{c:?} is offered on Windows");
        assert!(!app.tab().columns.shown(c), "{c:?} starts hidden");
    }
    app.tab_mut().columns.toggle(Column::Company);
    let [(tab, gen, ask, _, names)] = frame(&mut app).try_into().unwrap();
    let mut values = texts(["", "", "", ""]);
    let shell = ["14.11.2026 10:00", "Pixel 9", "Live", "Sunset", "Ann; Bob", "Contoso"];
    for (k, t) in shell.iter().enumerate() {
        values.text[4 + k] = (*t).to_owned();
    }
    app.on_shell_props(tab, gen, ask, vec![(names[0].clone(), values)]);
    let got: Vec<String> = extra.iter().map(|&c| cell(&app, "photo.jpg", c)).collect();
    assert_eq!(got, shell);
    assert_eq!(extra.map(|c| c.field()), [None; 6], "only the four columns of the plan sort");
}

#[test]
fn sorting_by_length_reads_every_file_in_small_batches_and_resorts_at_most_every_quarter_second_keeping_selection_and_focus() {
    let mut app = media_app();
    pick(&mut app, "song 123.mp3");
    app.sort_column(SortField::Duration);
    let mut asked = BTreeSet::new();
    let (mut changes, mut last) = (Vec::new(), shown_names(&app));
    let mut now = Instant::now();
    for _ in 0..400 {
        let asks = frame_at(&mut app, now);
        assert!(asks.iter().all(|a| a.4.len() <= 64), "batches stay small");
        assert!(asks.len() <= 2, "at most two batches wait in the shared queue");
        asked.extend(asks.iter().flat_map(|a| a.4.iter().cloned()));
        reply(&mut app, asks);
        let order = shown_names(&app);
        if order != last {
            changes.push(now);
            last = order;
        }
        assert_eq!(picked(&app), (vec!["song 123.mp3".to_owned()], Some("song 123.mp3".to_owned())), "selection and focus stay on the same file");
        now += Duration::from_millis(40);
    }
    assert_eq!(asked.len(), 400, "every file is read, not only the visible ones");
    assert!(changes.len() >= 2, "{changes:?}");
    assert!(changes.windows(2).all(|w| w[1] - w[0] >= Duration::from_millis(250)), "{changes:?}");
    let mut songs: Vec<(u64, String)> = (0..300).map(|k| format!("song {k:03}.mp3")).map(|n| (seconds_of(n.as_bytes()).unwrap(), n)).collect();
    songs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| sort::natural_cmp(a.1.as_bytes(), b.1.as_bytes(), CaseMode::Insensitive)));
    let mut want = vec!["Playlists".to_owned()];
    want.extend(songs.into_iter().map(|s| s.1));
    want.extend((0..100).map(|k| format!("notes {k:03}.txt")));
    assert_eq!(shown_names(&app), want, "files without a length go last");
    app.sort_column(SortField::Duration);
    frame_at(&mut app, now);
    assert_eq!(shown_names(&app).last().map(String::as_str), Some("notes 000.txt"), "and stay last when the order flips");
}

#[test]
fn without_sorting_or_grouping_by_a_property_only_visible_files_are_read() {
    let mut app = media_app();
    app.tab_mut().columns.toggle(Column::Duration);
    let mut asked = 0;
    for _ in 0..50 {
        let asks = frame(&mut app);
        asked += reply(&mut app, asks);
    }
    assert!(asked > 0 && asked < 100, "{asked}");
}

#[test]
fn grouping_by_length_reads_every_file_even_with_the_column_hidden() {
    let mut app = media_app();
    assert!(app.run_action("view.group.duration", Instant::now()));
    let (mut asked, mut now) = (0, Instant::now());
    for _ in 0..200 {
        let asks = frame_at(&mut app, now);
        asked += reply(&mut app, asks);
        now += Duration::from_millis(300);
    }
    assert_eq!(asked, 400);
    let v = app.tab().view.as_ref().unwrap();
    let l = v.listing();
    let keys: Vec<u64> = v.groups().iter().map(|g| sort::group_key(l, g.first, &v.snap().view).0).collect();
    assert_eq!(keys, [0, 1, 2, sort::UNSPECIFIED], "long, medium, short, then files and folders without a length");
}

#[test]
fn a_changed_file_drops_its_value_reads_it_again_and_ignores_a_reply_from_before_the_change() {
    let mut app = app_in(r"C:\Media", &[("a.mp3", false), ("b.mp3", false)]);
    frame(&mut app);
    app.tab_mut().columns.toggle(Column::Duration);
    let [(tab, gen, ask, _, _)] = frame(&mut app).try_into().unwrap();
    app.on_shell_props(tab, gen, ask, vec![(b"a.mp3".to_vec(), texts(["", "3:00", "", ""])), (b"b.mp3".to_vec(), texts(["", "1:00", "", ""]))]);
    assert_eq!(cell(&app, "a.mp3", Column::Duration), "3:00");
    let changed = DirChange { kind: ChangeKind::Modified, name: b"a.mp3".to_vec(), meta: Some(Meta { size: 9, ..Default::default() }) };
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![changed] }, Instant::now());
    assert!(frame(&mut app).is_empty());
    assert_eq!(cell(&app, "a.mp3", Column::Duration), "", "the old value is dropped");
    assert_eq!(cell(&app, "b.mp3", Column::Duration), "1:00", "other files keep theirs");
    let [(_, _, again, _, names)] = frame(&mut app).try_into().unwrap();
    assert_eq!(names, [b"a.mp3".to_vec()], "only the changed visible file is read again");
    app.on_shell_props(tab, gen, ask, vec![(b"a.mp3".to_vec(), texts(["", "3:00", "", ""]))]);
    assert_eq!(cell(&app, "a.mp3", Column::Duration), "", "a reply asked before the change is ignored");
    app.on_shell_props(tab, gen, again, vec![(b"a.mp3".to_vec(), texts(["", "4:00", "", ""]))]);
    assert_eq!(cell(&app, "a.mp3", Column::Duration), "4:00");
}

#[test]
fn a_changed_file_out_of_sight_is_read_again_only_while_the_list_sorts_by_its_property() {
    let mut app = media_app();
    app.tab_mut().columns.toggle(Column::Duration);
    let (tab, gen) = (app.tab().id, app.tab().gen);
    let mut now = Instant::now();
    let step = |app: &mut App, now: &mut Instant| {
        let asks = frame_at(app, *now);
        *now += Duration::from_millis(300);
        let seen: Vec<Vec<u8>> = asks.iter().flat_map(|a| a.4.iter().cloned()).collect();
        reply(app, asks);
        seen
    };
    for _ in 0..5 {
        step(&mut app, &mut now);
    }
    let far = b"song 299.mp3".to_vec();
    let change = |name: &[u8]| ProviderEvent::Changes { tab, gen, changes: vec![DirChange { kind: ChangeKind::Modified, name: name.to_vec(), meta: Some(Meta::default()) }] };
    app.on_provider_event(change(&far), now);
    for _ in 0..3 {
        assert!(!step(&mut app, &mut now).contains(&far), "an unseen file is not read while sorting by name");
    }
    app.sort_column(SortField::Duration);
    for _ in 0..100 {
        step(&mut app, &mut now);
    }
    app.on_provider_event(change(&far), now);
    let again = (0..5).any(|_| step(&mut app, &mut now).contains(&far));
    assert!(again, "while sorting by length the changed file is read again even out of sight");
}

#[test]
fn property_columns_read_nothing_inside_archives_the_recycle_bin_or_on_linux() {
    for path in [r"C:\Media\a.zip", "shell:RecycleBinFolder"] {
        let mut app = app_in(path, &[("photo.jpg", false)]);
        app.tab_mut().columns.toggle(Column::ImageSize);
        app.sort_column(SortField::Dimensions);
        assert!(frame(&mut app).is_empty(), "{path}");
    }
    let mut linux = app_in(r"C:\Media", &[("photo.jpg", false)]);
    linux.caps.unix_paths = true;
    linux.tab_mut().columns.toggle(Column::ImageSize);
    linux.sort_column(SortField::Dimensions);
    assert!(frame(&mut linux).is_empty());
    for c in SHELL_COLUMNS {
        assert!(!linux.column_kinds().contains(&c), "{c:?}");
    }
    for id in ["view.group.dimensions", "view.group.duration", "view.group.artist", "view.group.version"] {
        assert!(!linux.caps.supports(id), "{id}");
        assert!(crate::caps::Caps::default().supports(id), "{id}");
    }
}
