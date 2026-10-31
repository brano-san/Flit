use super::*;
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Persisted, Places};
use std::sync::Arc;
use std::time::Duration;

const SECTION_9: &[(&str, &[(&str, &str)])] = &[
    ("Quick", &[
        ("Toggle Subfolders", "Ctrl+E"),
        ("Toggle Inspector", "Space"),
        ("Split Right", "Ctrl+Shift+S"),
        ("Split Bottom", "Ctrl+S"),
        ("Open New Tab", "Ctrl+T"),
        ("Close Tab", "MouseMiddle, Ctrl+W, Ctrl+F4"),
        ("Options", "Ctrl+,"),
        ("Open File Explorer", ""),
        ("Open Console", ""),
        ("Toggle File Extension", ""),
        ("Toggle Hidden Files", "Ctrl+H"),
        ("Toggle System Files", ""),
    ]),
    ("Folder", &[
        ("Activate", "Enter, KeypadEnter"),
        ("Toggle File Cursor Selection", "Ctrl+Space"),
        ("Open In New Tab", "Ctrl+Enter"),
        ("Open In Right Split", "Ctrl+Shift+Enter"),
        ("Open In Bottom Split", ""),
        ("Open In New Window", ""),
        ("Open In Tree", "MouseLeft, Right"),
        ("Close In Tree", "MouseLeft, Left"),
        ("Open All In Tree", "Shift+MouseLeft, Shift+Right"),
        ("Close All In Tree", "Shift+MouseLeft, Shift+Left"),
        ("Copy", "Ctrl+C, Ctrl+Insert"),
        ("Cut", "Ctrl+X"),
        ("Paste", "Ctrl+V, Shift+Insert"),
        ("Paste Shortcut", ""),
        ("Copy Files To Previous Panel", ""),
        ("Copy Files To Next Panel", "Ctrl+F6"),
        ("Move Files To Previous Panel", ""),
        ("Move Files To Next Panel", "Shift+F6"),
        ("Trash", "Delete"),
        ("Delete", "Shift+Delete"),
        ("Duplicate", ""),
        ("Rename", "F2"),
        ("Inspect", "Alt+Enter, Alt+KeypadEnter"),
        ("Restore", ""),
        ("Copy As Path", "Ctrl+Shift+C"),
        ("Copy As Path (Smart)", ""),
        ("Create File", ""),
        ("Create Folder", "Ctrl+Shift+N"),
        ("Open Relative Path", "Ctrl+Shift+E"),
        ("Open Relative Path In Tab", "MouseMiddle"),
        ("Open Containing Path", ""),
        ("Open Containing Path In Tab", "MouseMiddle"),
        ("Cycle Folders Sort Order", ""),
        ("Toggle Folders", ""),
        ("Toggle Files", ""),
        ("Open Address Bar", "Ctrl+L, Alt+D"),
        ("Size All Columns To Fit", "Ctrl+="),
        ("Filter", "Ctrl+F, F3"),
        ("Search In Subfolders", "Ctrl+Shift+F"),
        ("Refresh Folder Stats", "Alt+R"),
        ("Refresh", "Ctrl+R, F5"),
        ("Back To Folder", "MouseX1, Alt+Left"),
        ("Forward To Folder", "MouseX2, Alt+Right"),
        ("Up To Folder", "Alt+Up, Backspace"),
        ("Empty Trash Items", ""),
        ("Restore Trash Items", ""),
        ("Cycle Folder View", ""),
        ("Toggle Folder Bookmark", ""),
        ("Open Folder Dropdown", "Alt+Down"),
        ("Open Left Folder Dropdown", "Alt+Left"),
        ("Open Right Folder Dropdown", "Alt+Right"),
        ("Open Types Dropdown", "Alt+T"),
    ]),
    ("View", &[
        ("Zoom In Content", "Ctrl+MouseWheelUp"),
        ("Zoom Out Content", "Ctrl+MouseWheelDown"),
        ("View Details", "Ctrl+Shift+1"),
        ("View Tree", "Ctrl+Shift+2"),
        ("View List", "Ctrl+Shift+3"),
        ("View Medium Icons", "Ctrl+Shift+4"),
        ("View Large Icons", "Ctrl+Shift+5"),
        ("View XL Icons", "Ctrl+Shift+6"),
    ]),
    ("Sort", &[
        ("Sort By Name", ""),
        ("Sort By Size", ""),
        ("Sort By Date Created", ""),
        ("Sort By Date Modified", ""),
        ("Sort By Date Accessed", ""),
        ("Sort By Date Deleted", ""),
        ("Sort By Total Items", ""),
        ("Sort By Folders", ""),
        ("Sort By Files", ""),
        ("Sort By Type", ""),
        ("Sort By Original Location", ""),
        ("Sort By Free Space", ""),
        ("Sort By File System", ""),
        ("Sort By Percent Full", ""),
        ("Sort By Volume Usage", ""),
        ("Sort By Folder Usage", ""),
    ]),
    ("Group By", &[
        ("Clear Grouping", ""),
        ("Group By Name", ""),
        ("Group By Size", ""),
        ("Group By Date Created", ""),
        ("Group By Date Modified", ""),
        ("Group By Date Accessed", ""),
        ("Group By Date Deleted", ""),
        ("Group By Total Items", ""),
        ("Group By Type", ""),
        ("Group By Original Location", ""),
        ("Group By Free Space", ""),
        ("Group By File System", ""),
        ("Group By Percent Full", ""),
        ("Group By Volume Usage", ""),
        ("Group By Folder Usage", ""),
        ("Collapse All Groups", ""),
        ("Expand All Groups", ""),
    ]),
    ("Inspector", &[
        ("Toggle Inspector Details", "Ctrl+I"),
        ("Toggle Inspector Layout", "Shift+Space"),
        ("Increase Contents Size", "Ctrl+MouseWheelUp"),
        ("Decrease Inspector Contents Size", "Ctrl+MouseWheelDown"),
        ("Toggle Inspector Word Wrap", ""),
        ("Toggle Inspector Line Numbers", ""),
        ("Toggle Inspector Whitespace", ""),
        ("Filter Inspector Content", ""),
        ("Toggle Inspector Image Grid", ""),
        ("Toggle inspector image scale", ""),
    ]),
    ("Panels", &[
        ("Select Next Panel", "Tab"),
        ("Select Previous Panel", "Shift+Tab"),
        ("Select Left Panel", ""),
        ("Select Right Panel", ""),
        ("Select Panel Above", ""),
        ("Select Panel Below", ""),
        ("Even Panel Siblings", ""),
        ("Even All Panels", ""),
    ]),
    ("Tabs", &[
        ("Duplicate Tab", ""),
        ("Duplicate Tab To New Window", "Ctrl+N"),
        ("Shift Tab Left", "Ctrl+Shift+PageUp"),
        ("Shift Tab Right", "Ctrl+Shift+PageDown"),
        ("Close Other Tabs", ""),
        ("Close Left Tabs", ""),
        ("Close Right Tabs", ""),
        ("Close All Tabs", "Ctrl+Shift+W"),
        ("Reopen Closed Tab", "Ctrl+Shift+T"),
        ("Select Next Tab", "Ctrl+Tab, Ctrl+PageDown"),
        ("Select Previous Tab", "Ctrl+Shift+Tab, Ctrl+PageUp"),
        ("Select 1st Tab", "Ctrl+1, Ctrl+Keypad1"),
        ("Select 2nd Tab", "Ctrl+2, Ctrl+Keypad2"),
        ("Select 3rd Tab", "Ctrl+3, Ctrl+Keypad3"),
        ("Select 4th Tab", "Ctrl+4, Ctrl+Keypad4"),
        ("Select 5th Tab", "Ctrl+5, Ctrl+Keypad5"),
        ("Select 6th Tab", "Ctrl+6, Ctrl+Keypad6"),
        ("Select 7th Tab", "Ctrl+7, Ctrl+Keypad7"),
        ("Select 8th Tab", "Ctrl+8, Ctrl+Keypad8"),
        ("Select Last Tab", "Ctrl+9, Ctrl+Keypad9"),
        ("Tab Menu", "MouseRight, Ctrl+Shift+Down"),
        ("Search Tabs", "Ctrl+Shift+A"),
    ]),
    ("Go To", &[
        ("GoTo Select In Panel", "Ctrl+Enter"),
        ("GoTo Move Back", "MouseX1, Alt+Left"),
        ("GoTo Move Forward", "MouseX2, Alt+Right"),
        ("GoTo Move Up", "Alt+Up"),
    ]),
    ("Misc", &[
        ("Create Config", ""),
        ("Open Config", ""),
        ("Menu", "F10"),
        ("Toggle Quick Access", "Ctrl+B"),
        ("Filter Quick Access", "Alt+F"),
        ("GoTo", "Ctrl+P, F4"),
        ("Commands", "Ctrl+Shift+P"),
        ("Context Menu", "Shift+F10, Menu"),
        ("Quick Commands", "MouseMiddle"),
        ("Clear Bookmarks", ""),
        ("Clear Recents", ""),
        ("Clear Folders Views", ""),
        ("Save Layout", ""),
        ("Load Layout", ""),
        ("Minimize Window", "Alt+M"),
        ("Toggle Fullscreen", "F11"),
        ("Close Window", "Alt+F4"),
    ]),
    ("Options", &[
        ("Increase Font Size", "Alt+MouseWheelUp, Ctrl+Shift+="),
        ("Decrease Font Size", "Alt+MouseWheelDown, Ctrl+-"),
        ("Reset Font Size", "Ctrl+0"),
        ("Increase Spacing", "Shift+Alt+MouseWheelUp"),
        ("Decrease Spacing", "Shift+Alt+MouseWheelDown"),
    ]),
    ("Rename", &[
        ("Insert Incremental ID", ""),
        ("Insert Current Date", ""),
        ("Insert File Create Date", ""),
        ("Insert File Last Modified Date", ""),
        ("Rename Previous File", "Shift+Tab"),
        ("Rename Next File", "Tab"),
    ]),
    ("Text", &[
        ("Move Left", "Left"),
        ("Extend Left", "Shift+Left"),
        ("Move Right", "Right"),
        ("Extend Right", "Shift+Right"),
        ("Move Left By Word", "Ctrl+Left"),
        ("Extend Left By Word", "Ctrl+Shift+Left"),
        ("Move Right By Word", "Ctrl+Right"),
        ("Extend Right By Word", "Ctrl+Shift+Right"),
        ("Move Home", "Home"),
        ("Extend Home", "Shift+Home"),
        ("Move End", "End"),
        ("Extend End", "Shift+End"),
        ("Delete Left", "Backspace, Shift+Backspace"),
        ("Delete Left By Word", "Ctrl+Backspace"),
        ("Delete Right", "Delete"),
        ("Delete Right By Word", "Ctrl+Delete"),
        ("Select All", "Ctrl+A"),
        ("Copy", "Ctrl+C"),
        ("Cut", "Ctrl+X"),
        ("Paste", "Ctrl+V, Shift+Insert"),
        ("Uppercase", ""),
        ("Lowercase", ""),
        ("Autocomplete", "Tab"),
        ("Undo", "Ctrl+Z"),
        ("Redo", "Ctrl+Shift+Z"),
    ]),
    ("Scroll", &[
        ("Select Previous", "Up"),
        ("Select Next", "Down"),
        ("Select Left", "Left"),
        ("Select Right", "Right"),
        ("Select First", "Home"),
        ("Select Last", "End"),
        ("Scroll Select Up", "PageUp, Ctrl+U"),
        ("Scroll Select Down", "PageDown, Ctrl+D"),
        ("Center Camera", ""),
        ("Move File Cursor To Previous", "Ctrl+Up, Ctrl+K"),
        ("Move File Cursor To Next", "Ctrl+Down, Ctrl+J"),
        ("Move File Cursor Left", "Ctrl+Left"),
        ("Move File Cursor Right", "Ctrl+Right"),
        ("Extend Select Previous", "Shift+Up"),
        ("Extend Select Next", "Shift+Down"),
        ("Extend Select Left", "Shift+Left"),
        ("Extend Select Right", "Shift+Right"),
        ("Extend Select First", "Shift+Home"),
        ("Extend Select Last", "Shift+End"),
        ("Scroll Extend Select Up", "Ctrl+Shift+U, Shift+PageUp"),
        ("Scroll Extend Select Down", "Ctrl+Shift+D, Shift+PageDown"),
        ("Select All", "Ctrl+A"),
    ]),
];

const GROUP_NAMES: [(Group, &str); 14] = [
    (Group::Quick, "Quick"),
    (Group::Folder, "Folder"),
    (Group::View, "View"),
    (Group::Sort, "Sort"),
    (Group::GroupBy, "Group By"),
    (Group::Inspector, "Inspector"),
    (Group::Panels, "Panels"),
    (Group::Tabs, "Tabs"),
    (Group::GoTo, "Go To"),
    (Group::Misc, "Misc"),
    (Group::Options, "Options"),
    (Group::Rename, "Rename"),
    (Group::Text, "Text"),
    (Group::Scroll, "Scroll"),
];

#[test]
fn the_registry_holds_every_command_of_section_9_with_its_group_and_shortcuts() {
    assert_eq!(SECTION_9.len(), GROUP_NAMES.len());
    for ((name, rows), (group, title)) in SECTION_9.iter().zip(GROUP_NAMES) {
        assert_eq!(*name, title);
        assert_eq!(group.title().en(), title);
        assert_eq!(Group::ALL.iter().position(|&g| g == group), GROUP_NAMES.iter().position(|&(g, _)| g == group), "{title} keeps its place");
        let ours: Vec<&Action> = ACTIONS.iter().filter(|a| a.group == group).collect();
        assert_eq!(ours.len(), rows.len(), "{title}: {:?}", ours.iter().map(|a| a.title.en()).collect::<Vec<_>>());
        for (a, (name, keys)) in ours.iter().zip(rows.iter()) {
            assert_eq!(a.title.en(), *name, "{title}");
            assert_eq!(keys_text(a), *keys, "{title} / {name}");
            assert!(!a.desc.en().is_empty() && a.desc.en().ends_with(['.', '!']), "{name}");
        }
    }
    assert!(ACTIONS.iter().filter(|a| a.group == Group::Flit).all(|a| a.rebindable()), "Flit-only commands live in the main context");
    assert_eq!(Group::ALL.last(), Some(&Group::Flit), "the Flit group comes after Scroll");
}

#[test]
fn defaults_never_use_a_bare_character_key() {
    for a in ACTIONS {
        for c in a.keys {
            let bare = matches!(c.key, Key::Char(_)) && !c.mods.ctrl && !c.mods.alt;
            assert!(!bare, "{} uses {}", a.id, chord_text(c));
        }
    }
}

#[test]
fn a_shortcut_belongs_to_one_command_per_context() {
    let mut seen: Vec<(Ctx, Chord, &str)> = Vec::new();
    for a in ACTIONS {
        for &c in a.keys {
            if let Some((_, _, other)) = seen.iter().find(|(x, k, _)| *x == a.ctx && *k == c) {
                panic!("{} and {} share {} in {:?}", other, a.id, chord_text(&c), a.ctx);
            }
            seen.push((a.ctx, c, a.id));
        }
        if a.rebindable() {
            for c in a.keys {
                assert_eq!(find(c.key, c.mods).map(|f| f.id), Some(a.id), "{}", chord_text(c));
            }
        }
    }
    let mut ids: Vec<&str> = ACTIONS.iter().map(|a| a.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), ACTIONS.len());
    let shared = |c: Chord| ACTIONS.iter().filter(|a| a.keys.contains(&c)).map(|a| a.ctx).collect::<Vec<_>>();
    assert_eq!(shared(alt(Key::Left)), [Ctx::Main, Ctx::Dropdown, Ctx::GoTo]);
    assert_eq!(shared(key(Key::Tab)), [Ctx::Main, Ctx::Rename, Ctx::Field]);
    assert_eq!(find(Key::Char('N'), Mods { ctrl: true, shift: false, alt: true }).map(|a| a.id), None, "AltGr is not Ctrl");
}

#[test]
fn unavailable_commands_keep_their_keys_but_do_nothing() {
    let mut app = crate::ui::harness::app(&["a.txt", "b.txt"]);
    for a in ACTIONS.iter().filter(|a| a.run.is_none()) {
        assert!(!app.available(a.id), "{}", a.id);
        assert!(!app.run_action(a.id, Instant::now()), "{}", a.id);
        assert!(a.ctx != Ctx::Main, "every command of the main context runs: {}", a.id);
    }
    press(&mut app, ctrl(Key::Char('E')));
    assert!(!app.search_active(), "Ctrl+E is Toggle Subfolders now, not search");
    assert!(app.tab().subfolders);
    press(&mut app, key(Key::F(4)));
    assert!(app.go_open(), "F4 opens GoTo");
}

#[test]
fn search_filter_and_address_follow_section_9() {
    let mut app = crate::ui::harness::app(&["a.txt", "b.txt"]);
    press(&mut app, ctrl_shift(Key::Char('F')));
    assert!(app.search_active(), "Ctrl+Shift+F opens the search in subfolders");
    press(&mut app, key(Key::Escape));
    assert!(!app.search_active());
    for c in [key(Key::F(3)), ctrl(Key::Char('F'))] {
        press(&mut app, c);
        assert_eq!(app.focus, crate::ui::Focus::Filter, "{}", chord_text(&c));
        press(&mut app, key(Key::Escape));
        app.focus = crate::ui::Focus::List;
    }
    for c in [ctrl(Key::Char('L')), alt(Key::Char('D'))] {
        press(&mut app, c);
        assert!(app.go_open(), "{}", chord_text(&c));
        press(&mut app, key(Key::Escape));
    }
}

#[test]
fn tab_cycles_panels_only_while_the_list_has_focus() {
    let mut app = crate::ui::harness::app(&["a.txt", "b.txt"]);
    assert!(app.run_action("panel.split_right", Instant::now()));
    let first = app.panels.active;
    app.focus = crate::ui::Focus::List;
    press(&mut app, key(Key::Tab));
    assert_ne!(app.panels.active, first, "Tab moves to the next panel");
    press(&mut app, shift(Key::Tab));
    assert_eq!(app.panels.active, first, "Shift+Tab comes back");
    press(&mut app, ctrl(Key::Char('F')));
    press(&mut app, key(Key::Tab));
    assert_eq!(app.panels.active, first, "in a text field Tab never changes the panel");
}

#[test]
fn new_commands_work_on_the_list() {
    let mut app = crate::ui::harness::app(&["a.txt", "b.txt", "c.txt"]);
    press(&mut app, key(Key::Down));
    press(&mut app, ctrl(Key::Char('J')));
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!((v.focus, v.selected_count()), (Some(1), 1), "Ctrl+J moves the cursor without selecting");
    press(&mut app, ctrl(Key::Space));
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 2);
    press(&mut app, ctrl(Key::Char('A')));
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 3);
    app.drain_effects().for_each(drop);
    assert!(app.run_action("item.copy_path_smart", Instant::now()));
    let copied: Vec<String> = app.drain_effects().filter_map(|e| if let Effect::SetClipboard(s) = e { Some(s) } else { None }).collect();
    assert_eq!(copied, vec![[r"C:\Work\a.txt", r"C:\Work\b.txt", r"C:\Work\c.txt"].join("\r\n")]);
    assert_eq!(smart_quote(r"C:\My Files"), r#""C:\My Files""#);
    assert_eq!(smart_quote(r"C:\Work"), r"C:\Work");
    let scale = app.ui_scale();
    press(&mut app, ctrl_shift(Key::Char('=')));
    assert_eq!(app.ui_scale(), scale + 10);
    press(&mut app, ctrl(Key::Char('-')));
    press(&mut app, ctrl(Key::Char('-')));
    assert_eq!(app.ui_scale(), scale - 10);
    press(&mut app, ctrl(Key::Char('0')));
    assert_eq!(app.ui_scale(), 100);
    let folders = app.tab().view_spec.sort.folders;
    for _ in 0..3 {
        app.run_action("view.sort.cycle_folders", Instant::now());
    }
    assert_eq!(app.tab().view_spec.sort.folders, folders, "three steps go round");
}

#[test]
fn tabs_shift_and_get_selected_by_number() {
    let mut app = crate::ui::harness::app(&["a.txt"]);
    for _ in 0..3 {
        app.run_action("tab.duplicate", Instant::now());
    }
    assert_eq!(app.tabs().len(), 4);
    press(&mut app, ctrl(Key::Char('2')));
    assert_eq!(app.cur, 1);
    press(&mut app, ctrl(Key::Char('9')));
    assert_eq!(app.cur, 3);
    let id = app.tab().id;
    press(&mut app, ctrl_shift(Key::PageUp));
    assert_eq!((app.cur, app.tab().id), (2, id));
    press(&mut app, ctrl_shift(Key::PageDown));
    press(&mut app, ctrl_shift(Key::PageDown));
    assert_eq!((app.cur, app.tab().id), (3, id), "the last tab stays last");
    press(&mut app, ctrl(Key::Char('8')));
    assert_eq!(app.cur, 3, "a missing tab is not selected");
}

#[test]
fn renamed_ids_lead_to_current_commands() {
    for (old, new) in RENAMED {
        assert!(index(old).is_none(), "{old}");
        assert!(index(new).is_some(), "{new}");
        assert_eq!(renamed(old), *new);
    }
    assert_eq!(renamed("file.copy"), "file.copy");
}

#[test]
fn every_action_runs_headless() {
    for a in ACTIONS {
        let now = Instant::now();
        let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (900, 500), now);
        app.set_places(Arc::new(Places::default()));
        let t = app.tab();
        let mut listing = Listing::default();
        let mut keys = SortKeys::new(CaseMode::Insensitive);
        for n in ["a.txt", "b.txt"] {
            let i = listing.push(n.as_bytes(), Meta::default());
            keys.push(listing.name(i));
        }
        let mut order = Vec::new();
        sort::sort(&listing, &keys, t.view_spec, &mut order);
        let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: t.view_spec, done: true, error: None, elapsed: Duration::ZERO };
        t.slot.publish(Arc::new(snap));
        app.poll();
        if let Some(run) = a.run {
            run(&mut app, now);
        }
        app.frame(&mut Gfx::new(Mono, 1024), now);
        assert!(!app.tabs().is_empty(), "{}", a.id);
    }
}

fn launched(app: &mut App) -> Vec<crate::provider::Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: crate::provider::ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

fn press(app: &mut App, c: Chord) {
    app.on_event(&crate::input::Event::Key { key: c.key, mods: c.mods, pressed: true }, Instant::now());
}

fn bare_app() -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (900, 500), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

#[test]
fn help_and_log_shortcuts_open_the_documentation_the_log_file_and_its_folder() {
    use crate::path::FsPath;
    use crate::provider::{Launch, ProviderRequest};
    let mut app = bare_app();
    press(&mut app, key(Key::F(1)));
    assert_eq!(launched(&mut app), [Launch::Run { program: "https://github.com/brano-san/Flit#readme".into(), args: String::new(), dir: None }]);
    press(&mut app, ctrl(Key::Char('.')));
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Provider { req: ProviderRequest::Open { .. }, .. })), "no log folder, nothing opens");
    assert_eq!(app.tab().error.as_deref(), Some("Flit has no log folder in this session"));
    app.about.logs = FsPath::parse(r"C:\Users\me\AppData\Local\Flit\logs");
    press(&mut app, ctrl(Key::Char('.')));
    let opened: Vec<FsPath> = app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } = e { Some(path) } else { None }).collect();
    assert_eq!(opened, [FsPath::parse(r"C:\Users\me\AppData\Local\Flit\logs\flit.log").unwrap()]);
    press(&mut app, ctrl_shift(Key::Char('.')));
    assert_eq!((app.tabs().len(), app.current_location().display().into_owned()), (2, r"C:\Users\me\AppData\Local\Flit\logs".to_owned()));
    for (id, url) in super::super::settings_page::LINKS {
        assert!(app.run_action(id, Instant::now()), "{id}");
        assert_eq!(launched(&mut app), [Launch::Run { program: url.into(), args: String::new(), dir: None }], "{id}");
        assert!(url.starts_with("https://github.com/brano-san/Flit"), "{url}");
    }
}

#[test]
fn shell_folders_flit_cannot_show_go_to_file_explorer_with_a_note() {
    use crate::provider::Launch;
    let mut app = bare_app();
    let open = crate::open::Open { paths: vec![b"shell:ControlPanelFolder".to_vec()], ..Default::default() };
    app.open(&open, &|_| None, Instant::now());
    assert_eq!(launched(&mut app), [Launch::Run { program: "explorer.exe".into(), args: "\"shell:ControlPanelFolder\"".into(), dir: None }]);
    assert_eq!(app.tab().error.as_deref(), Some("Flit cannot show shell:ControlPanelFolder, so it opens in File Explorer"));
    assert_eq!(app.tabs().len(), 1);
    let known = crate::open::Open { paths: vec![b"shell:Downloads".to_vec()], ..Default::default() };
    app.open(&known, &|_| Location::parse(r"C:\Users\me\Downloads"), Instant::now());
    assert!(launched(&mut app).is_empty(), "folders with a file system path still open in Flit");
    let smuggled = crate::open::Open { paths: vec![br#"shell:x" "C:\evil.exe"#.to_vec()], ..Default::default() };
    app.open(&smuggled, &|_| None, Instant::now());
    assert!(launched(&mut app).is_empty(), "a quote cannot add arguments to File Explorer");
}

#[test]
fn open_containing_path_asks_the_provider_and_follows_the_answer() {
    let mut app = crate::ui::harness::app(&["a.lnk", "b.txt"]);
    app.drain_effects().for_each(drop);
    assert!(app.run_action("item.open_containing", Instant::now()));
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Provider { .. })), "nothing is selected");
    press(&mut app, key(Key::Down));
    app.drain_effects().for_each(drop);
    assert!(app.run_action("item.open_containing_tab", Instant::now()));
    let launched = app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: crate::provider::ProviderRequest::Launch { launch: crate::provider::Launch::Reveal { item, new_tab }, .. }, .. } => Some((item.display().into_owned(), new_tab)),
        _ => None,
    });
    assert_eq!(launched, Some((r"C:\Work\a.lnk".to_owned(), true)));
    let tab = app.tab().id;
    app.on_provider_event(crate::provider::ProviderEvent::Reveal { tab, path: crate::path::FsPath::parse(r"D:\real\target.txt").unwrap(), new_tab: true }, Instant::now());
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.current_location().path().display(), r"D:\real");
}
