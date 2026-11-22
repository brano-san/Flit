use super::network::{saved_address, server_label};
use super::{App, SavedServer};
use crate::hash::FxHashSet;
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::render::{Color, Rect};
use crate::sort::{self, SortKeys};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use std::sync::Arc;
use std::time::Duration;

pub(super) fn draw_server_icon<B: TextBackend>(g: &mut Gfx<B>, listing: &Listing, i: u32, rect: Rect, color: Color) -> bool {
    if listing.saved_servers == 0 {
        return false;
    }
    let scheme = listing.name(i).split(|&b| b == b':').next().unwrap_or_default();
    let glyph = match scheme {
        b"ssh" | b"sftp" => "\u{E756}",
        b"smb" | b"afp" => "\u{E7F4}",
        b"dav" | b"davs" | b"dav+sd" | b"davs+sd" => "\u{E71B}",
        b"ftp" | b"ftps" => "\u{E896}",
        b"nfs" => super::G_DRIVE,
        _ => "\u{E72D}",
    };
    super::centered(g, rect, glyph, FontStyle::new(FontFamily::Icons, rect.h * 0.85, false), color);
    true
}

fn combined(raw: &DirSnapshot, saved: &[SavedServer]) -> DirSnapshot {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(raw.keys.case());
    let addresses: FxHashSet<&str> = saved.iter().map(|s| s.address.as_str()).collect();
    for server in saved {
        let i = listing.push(server.address.as_bytes(), Meta { attrs: attr::DIR, ..Default::default() });
        let label = server.name.as_deref().unwrap_or_else(|| server_label(&server.address));
        listing.set_label(i, label);
        keys.push(label.as_bytes());
    }
    listing.saved_servers = listing.len() as u32;
    for i in 0..raw.listing.len() as u32 {
        let name = raw.listing.name(i);
        if std::str::from_utf8(name).ok().and_then(saved_address).is_some_and(|uri| addresses.contains(uri.as_str())) {
            continue;
        }
        let new = listing.push(name, raw.listing.entries[i as usize].meta());
        if let Some(label) = raw.listing.labeled(i) {
            listing.set_label(new, label);
        }
        keys.push(listing.label(new));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, raw.view, &mut order);
    DirSnapshot { gen: raw.gen, path: raw.path.clone(), listing, keys, order, view: raw.view, done: raw.done, error: raw.error.clone(), elapsed: raw.elapsed }
}

impl App {
    pub(super) fn with_saved_servers(&mut self, k: usize, raw: Arc<DirSnapshot>) -> Arc<DirSnapshot> {
        if !self.caps.unix_paths || !raw.path.is_network() || !raw.path.is_root() || self.tabs[k].gen != raw.gen {
            return raw;
        }
        self.tabs[k].discovered_servers = Some(raw.clone());
        let saved = &self.nav.sidebar.prefs.saved_servers;
        if saved.is_empty() { raw } else { Arc::new(combined(&raw, saved)) }
    }

    pub(super) fn show_saved_servers(&mut self, k: usize) {
        let tab = &self.tabs[k];
        let path = tab.location().path();
        if !self.caps.unix_paths || !path.is_network() || !path.is_root() || tab.gen == 0 {
            return;
        }
        let raw = tab.discovered_servers.clone().unwrap_or_else(|| Arc::new(DirSnapshot {
            gen: tab.gen, path: path.clone(), listing: Listing::default(), keys: SortKeys::new(self.case), order: Vec::new(), view: tab.view_spec, done: false, error: None, elapsed: Duration::ZERO,
        }));
        let selection = tab.view.as_ref().filter(|v| v.snap().gen == tab.gen).map(|v| {
            let name = |i| v.listing().name(i).to_vec();
            (v.selected().map(name).collect::<Vec<_>>(), v.focus.map(name), v.anchor.map(name))
        });
        self.apply_snapshot(k, raw);
        if let Some(view) = self.tabs[k].view.as_mut().filter(|v| v.is_filtered()) {
            view.set_filter("");
            self.refilter_tab(k);
        }
        if let (Some((selected, focus, anchor)), Some(view)) = (selection, &mut self.tabs[k].view) {
            view.clear_selection();
            for name in selected {
                if let Some(i) = view.find(&name).filter(|&i| view.is_visible(i)) {
                    view.set_selected(i, true);
                }
            }
            view.focus = focus.and_then(|name| view.find(&name)).filter(|&i| view.is_visible(i));
            view.anchor = anchor.and_then(|name| view.find(&name)).filter(|&i| view.is_visible(i));
        }
    }

    pub(super) fn refresh_saved_servers(&mut self) {
        self.each_pane(Self::show_saved_servers);
        self.dirty = true;
    }
}
