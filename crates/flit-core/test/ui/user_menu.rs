use super::*;
use crate::input::{Event, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::provider::ProviderRequest;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::form::Control;
use crate::ui::{Effect, Persisted, ShellMenu};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn app(items: &[(&str, bool)]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
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
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn command(name: &str, icon: &str, command: &str) -> UserEntry {
    UserEntry { name: name.into(), icon: icon.into(), command: command.into(), ..UserEntry::default() }
}

fn entries() -> Vec<UserEntry> {
    let markdown = UserEntry { ext: ".md, MARKDOWN".into(), folders: false, ..command("Preview", "eye", "mdview {path}") };
    let here = UserEntry { files: false, folders: false, background: true, ..command("Terminal here", "terminal", "wt -d {dir}") };
    let tools = UserEntry { name: "Tools".into(), submenu: Some(vec![command("Hash", "", "hasher \"{name}\" {paths}")]), ..UserEntry::default() };
    vec![command("Code", "C:\\icons\\code.png", "code --goto {paths}"), markdown, here, tools]
}

fn right_click(a: &mut App, row: Option<usize>) -> ShellMenu {
    let l = &a.layout;
    let (x, y) = (l.cols[0].x + 200.0, row.map_or(l.list.y + 200.0, |r| l.list.y + (r as f32 + 0.5) * l.row_h));
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
    a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).unwrap()
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, .. } => vec![label.clone()],
            MenuItem::Submenu { label, items } => labels(items).into_iter().map(|l| format!("{label}/{l}")).collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn launched(a: &mut App, menu: &ShellMenu, label: &str) -> Launch {
    let k = labels(&menu.menu).iter().position(|l| l == label).unwrap_or_else(|| panic!("{label} missing"));
    a.on_event(&Event::MenuCommand(k), Instant::now());
    a.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } => Some(launch),
        _ => None,
    }).unwrap()
}

fn shown(p: &str) -> String {
    FsPath::parse(p).unwrap().display().into_owned()
}

#[test]
fn entries_persist_as_plain_json_and_reject_what_they_cannot_run() {
    let list = entries();
    let j = to_json(&list);
    assert_eq!(j[0], json!({ "name": "Code", "icon": "C:\\icons\\code.png", "command": "code --goto {paths}", "show": ["files", "folders"] }));
    assert_eq!(j[3]["items"][0]["command"], "hasher \"{name}\" {paths}");
    assert_eq!(from_json(&j), Some(list));
    assert!(from_json(&json!([{ "name": "x", "command": "a" }])).unwrap()[0].files, "files and folders by default");
    assert_eq!(from_json(&json!([{ "name": "x" }])), None, "a command needs a command line");
    assert_eq!(from_json(&json!([{ "name": "", "command": "a" }])), None);
    assert_eq!(from_json(&json!([{ "name": "s", "items": [{ "name": "t", "items": [] }] }])), None, "submenus nest one level");
    let doc = crate::ui::settings::parse(&json!({ "menu.custom": j }).to_string());
    assert_eq!(doc.value("menu.custom"), Some(&crate::ui::Value::Menu(entries())));
}

#[test]
fn entries_match_files_folders_background_and_extensions() {
    let list = entries();
    let file: &[(bool, &[u8])] = &[(false, b"notes.MD")];
    let mixed: &[(bool, &[u8])] = &[(false, b"a.txt"), (true, b"sub")];
    assert!(list[0].wants(Some(file)) && list[0].wants(Some(mixed)) && !list[0].wants(None));
    assert!(list[1].wants(Some(file)) && !list[1].wants(Some(&[(false, b"a.txt")])) && !list[1].wants(Some(&[(true, b"x.md")])));
    assert!(list[2].wants(None) && !list[2].wants(Some(file)));
}

#[test]
fn chosen_entries_launch_their_command_with_each_path_quoted_for_the_platform() {
    let mut a = app(&[("a b.txt", false), ("notes.md", false), ("sub", true)]);
    a.opts.user_menu = entries();
    let rows: Vec<String> = a.tab().view.as_ref().unwrap().rows().iter().map(|&i| String::from_utf8_lossy(a.tab().view.as_ref().unwrap().listing().name(i)).into_owned()).collect();
    let row = |n: &str| rows.iter().position(|r| r == n);
    let menu = right_click(&mut a, row("a b.txt"));
    let shown_labels = labels(&menu.menu);
    assert!(shown_labels.contains(&"Code".into()) && shown_labels.contains(&"Tools/Hash".into()), "{shown_labels:?}");
    assert!(!shown_labels.iter().any(|l| l == "Preview" || l == "Terminal here"), "{shown_labels:?}");
    let file = shown(r"C:\x\a b.txt");
    let want = Launch::Run { program: "code".into(), args: crate::cmdline::join(&["--goto".into(), file.clone()], crate::cmdline::POSIX), dir: FsPath::parse(r"C:\x") };
    assert_eq!(launched(&mut a, &menu, "Code"), want);
    let menu = right_click(&mut a, row("a b.txt"));
    let Launch::Run { program, args, .. } = launched(&mut a, &menu, "Tools/Hash") else { panic!() };
    assert_eq!((program.as_str(), args), ("hasher", crate::cmdline::join(&["a b.txt".into(), file], crate::cmdline::POSIX)));
    assert!(labels(&right_click(&mut a, row("notes.md")).menu).contains(&"Preview".into()));
    let background = right_click(&mut a, None);
    let Launch::Run { program, args, .. } = launched(&mut a, &background, "Terminal here") else { panic!() };
    assert_eq!((program.as_str(), args), ("wt", crate::cmdline::join(&["-d".into(), shown(r"C:\x")], crate::cmdline::POSIX)));
}

#[test]
fn image_icons_are_requested_once_from_the_file_through_the_icon_pipeline() {
    let mut a = app(&[("a.txt", false)]);
    let exe = UserEntry { icon: "C:\\tools\\app.exe".into(), ..command("App", "", "app") };
    a.opts.user_menu = vec![command("Code", "C:\\icons\\code.png", "code {path}"), exe];
    let menu = right_click(&mut a, Some(0));
    a.on_event(&Event::ShowMenu { items: menu.menu, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: Vec::new(), pending: false }, Instant::now());
    let mut g = Gfx::new(Mono, 256);
    a.frame(&mut g, Instant::now());
    a.frame(&mut g, Instant::now());
    let asked: Vec<(IconKind, String)> = a.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some((r.kind, r.path.display().into_owned())) } else { None }).collect();
    assert_eq!(asked, [(IconKind::Thumbnail, shown("C:\\icons\\code.png")), (IconKind::File, shown("C:\\tools\\app.exe"))]);
}

fn form_texts(a: &mut App, texts: &[(usize, &str)], choices: &[(usize, usize)]) {
    let f = a.jobs.form.as_mut().expect("the editor is open");
    for &(k, t) in texts {
        f.set_text(k, t);
    }
    for &(k, v) in choices {
        if let Some(Control::Choice { sel, .. }) = f.controls.get_mut(k) {
            *sel = v;
        }
    }
}

#[test]
fn the_options_editor_adds_moves_edits_and_deletes_entries() {
    let mut a = app(&[("a.txt", false)]);
    a.open_settings(Some(crate::ui::Page::Interface));
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert!(a.settings_lines().contains(&S::UserMenuGroup.t().to_owned()));
    a.new_user_entry(true);
    form_texts(&mut a, &[(NAME, "Dev")], &[]);
    a.form_submit(0);
    a.new_user_entry(false);
    form_texts(&mut a, &[(NAME, "Code"), (COMMAND, "  ")], &[(ICON, 1)]);
    a.form_submit(0);
    assert!(a.jobs.form.as_ref().is_some_and(|f| f.error), "an empty command keeps the editor open");
    form_texts(&mut a, &[(COMMAND, "code {paths}")], &[]);
    a.form_submit(0);
    assert_eq!(a.opts.user_menu[1], UserEntry { icon: GLYPHS[0].0.into(), ..command("Code", "", "code {paths}") });
    a.edit_user_entry(EntryAt { top: 1, child: None });
    form_texts(&mut a, &[(IMAGE, "\"D:\\i.png\"")], &[(PLACE, 1)]);
    a.form_submit(0);
    assert_eq!(a.opts.user_menu.len(), 1);
    assert_eq!(a.opts.user_menu[0].submenu.as_ref().map(|s| (s[0].name.as_str(), s[0].icon.as_str())), Some(("Code", "D:\\i.png")), "moved into the submenu with an image icon");
    a.new_user_entry(false);
    form_texts(&mut a, &[(NAME, "Second"), (COMMAND, "x")], &[]);
    a.form_submit(0);
    a.raise_user_entry(EntryAt { top: 1, child: None });
    assert_eq!(a.opts.user_menu.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Second", "Dev"]);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert!(a.settings_lines().iter().any(|l| l == "entry   Code"), "{:?}", a.settings_lines());
    a.delete_user_entry(EntryAt { top: 1, child: Some(0) });
    a.delete_user_entry(EntryAt { top: 0, child: None });
    assert_eq!(a.opts.user_menu.iter().map(|e| (e.name.as_str(), e.submenu.as_ref().map(Vec::len))).collect::<Vec<_>>(), [("Dev", Some(0))]);
}
