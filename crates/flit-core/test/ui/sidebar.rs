use super::*;
use crate::ui::Persisted;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn drive(letter: char) -> Drive {
    Drive { path: FsPath::parse(&format!("{letter}:\\")).unwrap(), label: format!("Disk ({letter}:)"), total: 100, free: 100 }
}

#[test]
fn pinned_items_get_labels_and_glyphs() {
    let mut sb = Sidebar::default();
    sb.state.pinned = vec![Location::computer(), loc("wsl://Debian/"), loc(r"C:\"), loc(r"C:\work\proj"), loc("wsl://Debian/home/u")];
    sb.places = Arc::new(Places { folders: vec![(KnownFolder::Music, loc(r"C:\Users\u\Music"))], drives: vec![drive('C')], wsl: vec!["Debian".into()], recycle_bin: false });
    sb.rebuild();
    let got: Vec<(&str, &str, Section)> = sb.items.iter().map(|it| (it.label.as_str(), it.glyph, it.section)).collect();
    assert_eq!(
        got,
        [
            ("This PC", G_DRIVE, Section::Pinned),
            ("Debian (WSL)", G_LINUX, Section::Pinned),
            ("C:", G_DRIVE, Section::Pinned),
            ("proj", G_FOLDER, Section::Pinned),
            ("u", G_FOLDER, Section::Pinned),
            ("Music", G_MUSIC, Section::Folders),
            ("Disk (C:)", G_DRIVE, Section::Drives),
            ("Debian", G_LINUX, Section::Wsl),
        ]
    );
    assert_eq!(sb.items[6].usage, Some(0.0));
    assert!(sb.is_pinned(&loc(r"C:\")));
    assert!(!sb.is_pinned(&loc(r"C:\Users\u\Music")));
}

#[test]
fn known_folders_map_to_shell_names() {
    let names: Vec<&str> = KnownFolder::ALL.iter().map(|k| k.shell_name()).collect();
    assert_eq!(names, ["shell:Profile", "shell:Desktop", "shell:Downloads", "shell:Personal", "shell:My Pictures", "shell:My Music", "shell:My Video"]);
    assert_eq!(KnownFolder::ALL.map(|k| k.label())[0], "Home");
}

#[test]
fn layout_shows_the_pin_hint_and_scrolls_long_lists() {
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (900, 400), Instant::now());
    assert!(app.nav.sidebar.hint.is_some());
    assert!(app.nav.sidebar.rects.is_empty());
    let drives = ('C'..='Z').map(drive).collect();
    app.set_places(Arc::new(Places { drives, ..Default::default() }));
    let sb = &app.nav.sidebar;
    assert_eq!(sb.rects.len(), 24);
    assert!(sb.content_h > sb.rect.h);
    let first = sb.rects[0].y;
    app.hover = Hit::Side(0);
    app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -2.0 }, Instant::now());
    assert!(app.nav.sidebar.rects[0].y < first, "the wheel scrolls the sidebar, not the list");
    app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -1000.0 }, Instant::now());
    let sb = &app.nav.sidebar;
    assert!((sb.rects.last().unwrap().bottom() + (6.0) - sb.rect.bottom()).abs() < 1.0, "clamped at the end");
    assert!(matches!(app.sidebar_hit(sb.rects[23].x + 5.0, sb.rects[23].y + 5.0), Some(Hit::Side(23))));
    assert!(app.sidebar_hit(sb.rect.right() + 50.0, sb.rect.y + 5.0).is_none());
}
