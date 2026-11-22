use super::*;
use crate::ui::harness::{app, frame};

#[test]
fn double_click_returns_a_file_instead_of_launching_it() {
    let mut app = app(&["file.png", "other.txt"]);
    app.start_picker(Request::default());
    app.click_row(0, MouseButton::Left, 2, Instant::now());
    let effects: Vec<_> = app.drain_effects().collect();
    assert!(effects.iter().any(|e| matches!(e, Effect::Picked { paths, .. } if paths[0].file_name() == b"file.png")));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: crate::provider::ProviderRequest::Open { .. }, .. })));
}

#[test]
fn file_picker_filter_hides_non_matches_and_escape_cancels() {
    let mut app = app(&["file.png", "other.txt"]);
    let request = Request::from_cli(vec![b"--filter".to_vec(), b"*.png".to_vec()]).unwrap();
    app.start_picker(request);
    assert_eq!(app.tab().view.as_ref().unwrap().rows().len(), 1);
    app.on_event(&Event::Key { key: Key::Escape, pressed: true, mods: Default::default() }, Instant::now());
    let effects: Vec<_> = app.drain_effects().collect();
    assert!(effects.iter().any(|e| matches!(e, Effect::Window(crate::ui::WindowCmd::Close))));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Picked { .. })));
}

#[test]
fn down_and_enter_choose_a_file_with_the_keyboard() {
    let mut app = app(&["file.png"]);
    app.start_picker(Request::default());
    let now = Instant::now();
    for key in [Key::Down, Key::Enter] {
        app.on_event(&Event::Key { key, pressed: true, mods: Default::default() }, now);
    }
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Picked { paths, .. } if paths[0].file_name() == b"file.png")));
}

#[test]
fn narrow_save_dialog_exposes_the_name_and_confirms_replacing_an_existing_file() {
    let mut app = app(&["file.png"]);
    app.start_picker(Request::from_cli(vec![b"--save".to_vec(), b"file.png".to_vec()]).unwrap());
    let now = Instant::now();
    app.on_event(&Event::Resized { width: 540, height: 400 }, now);
    frame(&mut app);
    let tree = app.accessibility();
    let name = tree.find(Control::Name.id()).unwrap();
    let save = tree.find(Control::Accept.id()).unwrap();
    assert_eq!(tree.nodes[name].value, "file.png");
    assert!(tree.nodes[name].bounds.bottom() <= tree.nodes[save].bounds.y);
    assert!(tree.nodes[name].bounds.w >= 200.0);
    app.on_event(&Event::Invoke(Control::Accept.id()), now);
    assert!(app.dialogs.confirm.is_some());
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Picked { .. })));
    app.on_event(&Event::Key { key: Key::Enter, pressed: true, mods: Default::default() }, now);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Picked { paths, .. } if paths[0].file_name() == b"file.png")));
}

#[test]
fn picker_filters_a_directory_by_file_type_and_keeps_unclassified_files_visible() {
    let mut app = app(&["README", "photo.jpg", "unknown"]);
    let mut snap = (**app.tab().view.as_ref().unwrap().snap()).clone();
    let classify = |name: &[u8]| match name {
        b"README" => Some(vec!["text/x-readme".into(), "text/plain".into()]),
        b"photo.jpg" => Some(vec!["image/jpeg".into()]),
        _ => None,
    };
    for i in 0..snap.listing.len() as u32 {
        if let Some(types) = classify(snap.listing.name(i)) {
            snap.listing.picker_mime.insert(i, types.into());
        }
    }
    app.apply_snapshot(0, Arc::new(snap));
    app.start_picker(Request {
        filters: vec![Filter { label: "Text".into(), patterns: Vec::new(), mime_types: vec!["text/plain".into()] }],
        ..Request::default()
    });
    let view = app.tab().view.as_ref().unwrap();
    let visible: Vec<_> = view.rows().iter().map(|&i| view.listing().name(i)).collect();
    assert_eq!(visible, [b"README".as_slice(), b"unknown"]);
}
