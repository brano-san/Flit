use super::{layouts, App, Effect, LayoutMode};
use crate::listing::{attr, DirSnapshot, Listing};
use crate::path::{FsPath, PathKind};
use crate::provider::{GenToken, TabId};
use std::sync::{Arc, Weak};

pub(super) struct ViewRules {
    pub network: Option<LayoutMode>,
    pub removable: Option<LayoutMode>,
    pub archive: Option<LayoutMode>,
    pub pictures_min: u8,
    pub pictures: LayoutMode,
    pub search_grid: bool,
}

impl Default for ViewRules {
    fn default() -> Self {
        Self { network: None, removable: None, archive: None, pictures_min: 0, pictures: LayoutMode::Grid, search_grid: false }
    }
}

impl ViewRules {
    pub(super) fn location_rules(&self) -> bool {
        self.network.is_some() || self.removable.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PictureShare {
    pictures: usize,
    files: usize,
}

impl PictureShare {
    fn reaches(self, percent: u8) -> bool {
        self.files > 0 && self.pictures as u64 * 100 >= self.files as u64 * u64::from(percent)
    }
}

pub fn count_pictures(listing: &Listing, current: &GenToken, ask: u64) -> Option<PictureShare> {
    let mut share = PictureShare::default();
    for (i, entry) in listing.entries.iter().enumerate() {
        if i % 256 == 0 && !current.is_current(ask) {
            return None;
        }
        if !entry.is_dir() && entry.attrs & attr::STREAM == 0 {
            share.files += 1;
            let ext = listing.ext(i as u32);
            share.pictures += usize::from(layouts::is_picture(ext) || crate::preview::IMAGES.iter().any(|s| ext.eq_ignore_ascii_case(s.as_bytes())));
        }
    }
    current.is_current(ask).then_some(share)
}

pub(super) struct PictureScan {
    snap: Weak<DirSnapshot>,
    ask: u64,
    cancel: GenToken,
    share: Option<PictureShare>,
}

impl Drop for PictureScan {
    fn drop(&mut self) {
        self.cancel.set(0);
    }
}

fn below(path: &FsPath, root: &FsPath) -> bool {
    let (p, prefix) = (path.as_bytes(), root.as_bytes());
    if p.len() < prefix.len() {
        return false;
    }
    let same = if root.kind() == PathKind::Unix { p.starts_with(prefix) } else { p[..prefix.len()].eq_ignore_ascii_case(prefix) };
    let sep: &[u8] = if root.kind() == PathKind::Unix { b"/" } else { b"\\" };
    same && (p.len() == prefix.len() || prefix.ends_with(sep) || p[prefix.len()..].starts_with(sep))
}

impl App {
    pub(super) fn refresh_view_rules(&mut self) {
        self.each_pane(|a, k| {
            if a.tabs.items[k].view.as_ref().is_some_and(|v| v.snap().gen == a.tabs.items[k].gen && v.snap().done) {
                a.settle_look(k, false, true);
            }
        });
        self.dirty = true;
    }

    pub(super) fn rule_layout(&mut self, k: usize, done: bool) -> Option<LayoutMode> {
        let t = &self.tabs.items[k];
        let rules = &self.settings.opts.view_rules;
        if !rules.location_rules() && rules.archive.is_none() && rules.pictures_min == 0 {
            return None;
        }
        if self.prefs.same_for_all || self.has_saved_view(&t.view_loc) {
            return None;
        }
        let v = t.view.as_ref()?;
        let path = t.view_loc.path();
        if let Some(mode) = rules.archive {
            if v.listing().archived || v.listing().entries.first().is_some_and(|e| e.attrs & attr::ARCHIVED != 0) {
                return Some(mode);
            }
        }
        if rules.location_rules() {
            let drive = self.nav.sidebar.places.drives.iter().filter(|d| !d.unmounted && below(path, &d.path)).max_by_key(|d| d.path.as_bytes().len());
            let network = path.kind() == PathKind::Unc || path.is_network()
                || path.as_bytes().get(..8).is_some_and(|p| p.eq_ignore_ascii_case(br"\\?\UNC\"))
                || drive.is_some_and(|d| d.remote.is_some());
            if let Some(mode) = rules.network.filter(|_| network) {
                return Some(mode);
            }
            if let Some(mode) = rules.removable.filter(|_| drive.is_some_and(|d| d.removable)) {
                return Some(mode);
            }
        }
        if rules.pictures_min == 0 || !done || !v.snap().done || v.snap().error.is_some()
            || !matches!(path.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim | PathKind::Unix)
        {
            return None;
        }
        let snap = v.snap().clone();
        let weak = Arc::downgrade(&snap);
        if let Some(scan) = t.picture_scan.as_ref().filter(|s| s.snap.ptr_eq(&weak)) {
            return scan.share.filter(|s| s.reaches(rules.pictures_min)).map(|_| rules.pictures);
        }
        let ask = self.gen();
        let cancel = GenToken::default();
        cancel.set(ask);
        let t = &mut self.tabs.items[k];
        t.picture_scan = Some(PictureScan { snap: weak, ask, cancel: cancel.clone(), share: None });
        self.effects.push(Effect::ViewPictures { tab: t.id, ask, snap, cancel });
        None
    }

    pub fn on_picture_share(&mut self, tab: TabId, ask: u64, share: PictureShare) {
        if let Some(id) = self.parked_of(tab) {
            return self.visit(id, |a| a.on_picture_share(tab, ask, share));
        }
        let Some(k) = self.tabs.items.iter().position(|t| t.id == tab) else { return };
        let t = &mut self.tabs.items[k];
        let Some(v) = t.view.as_ref().filter(|v| v.snap().gen == t.gen) else { return };
        let Some(scan) = t.picture_scan.as_mut().filter(|s| s.ask == ask && s.snap.ptr_eq(&Arc::downgrade(v.snap()))) else { return };
        scan.share = Some(share);
        self.settle_look(k, false, true);
        self.layout();
        self.dirty = true;
    }
}
