use super::*;
use crate::input::{Key, Mods, MouseButton};
use crate::provider::ProviderRequest;
use crate::text::tests::Mono;
use crate::ui::{Effect, Persisted};
use crate::{FsPath, Location};

fn cmd(label: &str) -> MenuItem {
    MenuItem::Command { label: label.into(), keys: String::new(), checked: false }
}

fn app() -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn shown(app: &mut App, launches: Vec<Launch>) {
    let items = vec![cmd("Copy path"), MenuItem::Separator, MenuItem::Submenu { label: "Open with".into(), items: vec![cmd("Editor"), cmd("Viewer")] }];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches }, Instant::now());
    let mut g = Gfx::new(Mono, 256);
    app.frame(&mut g, Instant::now());
}

fn key(app: &mut App, key: Key) {
    app.on_event(&Event::Key { key, mods: Mods::default(), pressed: true }, Instant::now());
}

#[test]
fn commands_are_counted_through_submenus() {
    assert_eq!(commands(&[cmd("a"), MenuItem::Separator, MenuItem::Submenu { label: "s".into(), items: vec![cmd("b"), cmd("c")] }]), 3);
}

#[test]
fn escape_and_clicks_outside_close_the_menu_without_a_command() {
    let mut app = app();
    shown(&mut app, Vec::new());
    assert!(app.own_menu_open());
    key(&mut app, Key::Escape);
    assert!(!app.own_menu_open());
    shown(&mut app, Vec::new());
    app.on_event(&Event::MouseDown { x: 900.0, y: 550.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    assert!(!app.own_menu_open());
    assert!(app.drain_effects().next().is_none());
}

#[test]
fn trailing_commands_launch_their_programs_and_the_rest_are_flit_commands() {
    let mut app = app();
    let editor = Launch::With { program: FsPath::parse("/usr/share/applications/editor.desktop").unwrap(), args: vec![FsPath::parse("/x/a.txt").unwrap()] };
    let viewer = Launch::With { program: FsPath::parse("/usr/share/applications/viewer.desktop").unwrap(), args: Vec::new() };
    shown(&mut app, vec![editor.clone(), viewer]);
    for k in [Key::Down, Key::Down, Key::Right, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    let launched: Vec<Launch> = app.drain_effects().filter_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } => Some(launch),
        _ => None,
    }).collect();
    assert_eq!(launched, [editor]);
    shown(&mut app, Vec::new());
    key(&mut app, Key::Down);
    key(&mut app, Key::Enter);
    assert!(!app.own_menu_open());
}
