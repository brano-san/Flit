use super::*;
use crate::input::{Event, Key, Mods};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::Persisted;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn app(items: &[&str]) -> App {
    let mut app = App::new(Persisted { path: Location::parse("C:\\Work").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in items {
        let i = listing.push(n.as_bytes(), Meta { size: 2048, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    frame(&mut app);
    app.drain_effects().for_each(drop);
    app
}

pub(super) fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

#[allow(dead_code)]
pub(super) fn press(app: &mut App, key: Key, shift: bool) {
    app.on_event(&Event::Key { key, mods: Mods { shift, ..Mods::default() }, pressed: true }, Instant::now());
    frame(app);
}