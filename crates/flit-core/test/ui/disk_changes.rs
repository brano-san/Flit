use super::*;
use crate::location::Location;
use crate::ui::Persisted;
use std::time::Instant;

#[test]
fn access_denied_requires_an_explicit_confirmation_before_elevation() {
    let state = Persisted { path: Location::parse(r"C:\photos").unwrap(), ..Default::default() };
    let mut app = App::new(state, Vec::new(), 0, false, 1.0, (490, 360), Instant::now());
    app.disk_changes();
    app.form_submit(0);
    let id = app.drain_effects().find_map(|effect| match effect {
        Effect::DiskChanges { id, request: Request::Read { elevated: false, .. } } => Some(id),
        _ => None,
    }).unwrap();
    app.on_disk_changes(id, Reply::Read(Err(Error::Denied)));
    assert!(!app.drain_effects().any(|effect| matches!(effect, Effect::DiskChanges { .. })));
    let form = app.jobs.form.as_ref().unwrap();
    assert_eq!(form.focus, form.controls.len() + 1);
    app.form_submit(0);
    assert!(app.drain_effects().any(|effect| matches!(effect, Effect::DiskChanges { request: Request::Read { elevated: true, .. }, .. })));
}
