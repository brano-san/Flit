use flit_core::archive::Defaults;
use flit_core::sort::SortField;
use flit_core::ui::{Column, FolderView, FolderViews, LayoutMode, NavState, PaneState, Persisted, Sizes, ViewPrefs};
use flit_core::Location;
use flit_platform::{Placement, Platform};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const VERSION: u32 = 2;
const STAGE_ONE_COLUMNS: [Column; 3] = [Column::Modified, Column::Type, Column::Size];

type Bounds = (i32, i32, i32, i32, bool);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Head {
    version: u32,
    path: Location,
    sort: SortField,
    descending: bool,
    show_hidden: bool,
    col_w: [f32; 3],
    placement: Option<Bounds>,
}

#[derive(Serialize, Deserialize)]
struct ViewRecord {
    view: FolderView,
    prefs: ViewPrefs,
    folders: Vec<(u64, FolderView)>,
}

type Look = (LayoutMode, Sizes);

#[derive(Serialize, Deserialize)]
struct LayoutRecord {
    view: Look,
    defaults: Look,
    folders: Vec<Look>,
    thumbnails: bool,
    thumb_cache_mb: u32,
}

fn look(v: &FolderView) -> Look {
    (v.layout, v.sizes)
}

fn with_look(v: FolderView, (layout, sizes): Look) -> FolderView {
    FolderView { layout, sizes, ..v }
}

fn legacy(v: FolderView) -> FolderView {
    FolderView { layout: LayoutMode::Details, ..v }
}

#[derive(Clone, Debug)]
pub struct Saved {
    version: u32,
    pub persisted: Persisted,
    placement: Option<Bounds>,
    pub nav: NavState,
    pub pane: PaneState,
}

impl Saved {
    pub fn new(persisted: Persisted, placement: Option<Placement>) -> Self {
        Self { version: VERSION, persisted, placement: placement.map(|p| (p.x, p.y, p.width, p.height, p.maximized)), nav: NavState::default(), pane: PaneState::default() }
    }

    pub fn with_nav(mut self, nav: NavState) -> Self {
        self.nav = nav;
        self
    }

    pub fn with_pane(mut self, pane: PaneState) -> Self {
        self.pane = pane;
        self
    }

    fn sanitized(mut self) -> Self {
        let p = &mut self.persisted;
        p.view = p.view.sanitized();
        p.prefs = p.prefs.clone().sanitized();
        self.placement = self.placement.filter(|&(_, _, w, h, _)| w > 0 && h > 0);
        self.nav = self.nav.sanitized();
        self.pane = self.pane.sanitized();
        self
    }

    fn head(&self) -> Head {
        let (p, v) = (&self.persisted, &self.persisted.view);
        let sort = if matches!(v.sort.field, SortField::Created | SortField::Tag) { SortField::Name } else { v.sort.field };
        Head { version: self.version, path: p.path.clone(), sort, descending: v.sort.descending, show_hidden: p.show_hidden, col_w: STAGE_ONE_COLUMNS.map(|c| v.columns.width(c)), placement: self.placement }
    }

    fn from_head(h: Head) -> Self {
        let mut view = FolderView::default();
        (view.sort.field, view.sort.descending) = (h.sort, h.descending);
        for (c, w) in STAGE_ONE_COLUMNS.into_iter().zip(h.col_w) {
            view.columns.widths[c as usize] = w;
        }
        let persisted = Persisted { path: h.path, show_hidden: h.show_hidden, view, ..Default::default() };
        Self { version: h.version, persisted, placement: h.placement, nav: NavState::default(), pane: PaneState::default() }
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (head, tail) = postcard::take_from_bytes::<Head>(bytes).ok()?;
        let mut saved = Self::from_head(head);
        let Ok((nav, tail)) = postcard::take_from_bytes::<NavState>(tail) else { return Some(saved) };
        saved.nav = nav;
        let Ok((mut r, tail)) = postcard::take_from_bytes::<ViewRecord>(tail) else { return Some(saved) };
        let mut rest = None;
        if let Ok((l, tail)) = postcard::take_from_bytes::<LayoutRecord>(tail) {
            (r.view, r.prefs.defaults) = (with_look(r.view, l.view), with_look(r.prefs.defaults, l.defaults));
            (r.prefs.thumbnails, r.prefs.thumb_cache_mb) = (l.thumbnails, l.thumb_cache_mb);
            if l.folders.len() == r.folders.len() {
                r.folders.iter_mut().zip(l.folders).for_each(|((_, v), lk)| *v = with_look(*v, lk));
            }
            if let Ok((pane, tail)) = postcard::take_from_bytes::<PaneState>(tail) {
                saved.pane = pane;
                rest = Some(tail);
            }
        }
        let p = &mut saved.persisted;
        (p.view, p.prefs, p.folders) = (r.view, r.prefs, FolderViews::from_entries(r.folders));
        if let Some((archive, _)) = rest.and_then(|t| postcard::take_from_bytes::<Defaults>(t).ok()) {
            p.archive = archive.sanitized();
        }
        Some(saved)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let p = &self.persisted;
        let folders = p.folders.entries();
        let prefs = ViewPrefs { defaults: legacy(p.prefs.defaults), ..p.prefs.clone() };
        let layouts = LayoutRecord {
            view: look(&p.view),
            defaults: look(&p.prefs.defaults),
            folders: folders.iter().map(|(_, v)| look(v)).collect(),
            thumbnails: p.prefs.thumbnails,
            thumb_cache_mb: p.prefs.thumb_cache_mb,
        };
        let views = ViewRecord { view: legacy(p.view), prefs, folders: folders.into_iter().map(|(k, v)| (k, legacy(v))).collect() };
        let mut bytes = postcard::to_stdvec(&self.head()).ok()?;
        bytes.extend(postcard::to_stdvec(&self.nav).ok()?);
        bytes.extend(postcard::to_stdvec(&views).ok()?);
        bytes.extend(postcard::to_stdvec(&layouts).ok()?);
        bytes.extend(postcard::to_stdvec(&self.pane).ok()?);
        bytes.extend(postcard::to_stdvec(&p.archive.clone().sanitized()).ok()?);
        Some(bytes)
    }

    pub fn placement(&self) -> Option<Placement> {
        self.placement.map(|(x, y, width, height, maximized)| Placement { x, y, width, height, maximized })
    }
}

fn file<P: Platform>() -> Option<PathBuf> {
    P::state_dir().map(|d| d.join("state.bin"))
}

pub fn load<P: Platform>() -> Saved {
    file::<P>()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| Saved::decode(&b))
        .filter(|s| s.version == VERSION)
        .map(Saved::sanitized)
        .unwrap_or_else(|| Saved::new(Persisted { path: Location::from_local(P::home()), ..Default::default() }, None))
}

pub fn save<P: Platform>(s: &Saved) {
    let Some(f) = file::<P>() else { return };
    let Some(bytes) = s.encode() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = f.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &f);
    }
}

#[cfg(test)]
#[path = "../test/state.rs"]
mod tests;
