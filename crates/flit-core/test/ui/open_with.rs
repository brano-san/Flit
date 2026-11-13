use super::*;
use crate::provider::ProviderRequest;
use crate::ui::{Effect, Persisted};
use crate::Location;
use std::time::Instant;

fn app() -> App {
    let mut app = App::new(Persisted { path: Location::parse("/home/u").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn choice(name: &str, file: &str) -> AppChoice {
    AppChoice { name: name.into(), program: p(&format!("/usr/share/applications/{file}")) }
}

fn launches(app: &mut App) -> Vec<Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

fn chooser(app: &mut App) {
    let apps = vec![choice("Viewer", "viewer.desktop"), choice("editor", "editor.desktop"), choice("Archiver", "ark.desktop"), choice("Viewer", "viewer.desktop")];
    app.launch(Launch::ChooseApp { files: vec![p("/home/u/a.txt")], mime: "text/plain".into(), apps });
}

fn titles(app: &App) -> Vec<String> {
    app.nav.palette.as_ref().expect("the chooser is open").titles.clone()
}

#[test]
fn other_application_opens_a_sorted_chooser_instead_of_launching() {
    let mut app = app();
    chooser(&mut app);
    assert!(launches(&mut app).is_empty());
    let rows = titles(&app);
    assert_eq!(rows[2..], ["Archiver", "editor", "Viewer"]);
    assert!(rows[0].contains("text/plain"), "{rows:?}");
    assert!(matches!(app.nav.palette.as_ref().unwrap().pick, Pick::OpenWith(_)));
}

#[test]
fn typing_filters_the_applications_and_enter_opens_the_files_with_the_match() {
    let mut app = app();
    chooser(&mut app);
    app.palette_insert("view");
    let p = app.nav.palette.as_ref().unwrap();
    assert_eq!(p.titles[p.matches[0]], "Viewer");
    app.run_palette_item(0, Instant::now());
    assert_eq!(launches(&mut app), [Launch::With { program: self::p("/usr/share/applications/viewer.desktop"), args: vec![self::p("/home/u/a.txt")] }]);
    assert!(app.nav.palette.is_none());
}

#[test]
fn the_default_toggle_stays_open_and_makes_the_pick_the_default() {
    let mut app = app();
    chooser(&mut app);
    app.run_palette_item(0, Instant::now());
    let c = match &app.nav.palette.as_ref().expect("the toggle keeps the chooser open").pick {
        Pick::OpenWith(c) => c.clone(),
        other => panic!("{other:?}"),
    };
    assert!(c.remember);
    assert_eq!(app.nav.palette.as_ref().unwrap().sel, 0);
    app.run_palette_item(3, Instant::now());
    let editor = self::p("/usr/share/applications/editor.desktop");
    assert_eq!(launches(&mut app), [Launch::SetDefault { program: editor.clone(), mime: "text/plain".into() }, Launch::With { program: editor, args: vec![self::p("/home/u/a.txt")] }]);
}

#[test]
fn a_custom_command_is_typed_in_its_own_field() {
    let mut app = app();
    chooser(&mut app);
    app.run_palette_item(1, Instant::now());
    assert!(matches!(app.nav.palette.as_ref().unwrap().pick, Pick::OpenWithCommand(_)));
    app.run_palette_item(0, Instant::now());
    assert!(launches(&mut app).is_empty(), "an empty command runs nothing");
    app.palette_insert("gimp --new %F");
    app.run_palette_item(0, Instant::now());
    assert_eq!(launches(&mut app), [Launch::Custom { command: "gimp --new %F".into(), files: vec![self::p("/home/u/a.txt")], mime: "text/plain".into(), remember: false }]);
}
