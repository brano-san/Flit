use flit_core::sort::SortField;
use flit_core::ui::{Column, FolderView, FolderViews, NavState, Persisted, ViewPrefs};
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

#[derive(Clone, Debug)]
pub struct Saved {
    version: u32,
    pub persisted: Persisted,
    placement: Option<Bounds>,
    pub nav: NavState,
}

impl Saved {
    pub fn new(persisted: Persisted, placement: Option<Placement>) -> Self {
        Self { version: VERSION, persisted, placement: placement.map(|p| (p.x, p.y, p.width, p.height, p.maximized)), nav: NavState::default() }
    }

    pub fn with_nav(mut self, nav: NavState) -> Self {
        self.nav = nav;
        self
    }

    fn sanitized(mut self) -> Self {
        let p = &mut self.persisted;
        p.view = p.view.sanitized();
        p.prefs.defaults = p.prefs.defaults.sanitized();
        self.placement = self.placement.filter(|&(_, _, w, h, _)| w > 0 && h > 0);
        self.nav = self.nav.sanitized();
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
        Self { version: h.version, persisted, placement: h.placement, nav: NavState::default() }
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (head, tail) = postcard::take_from_bytes::<Head>(bytes).ok()?;
        let mut saved = Self::from_head(head);
        let Ok((nav, tail)) = postcard::take_from_bytes::<NavState>(tail) else { return Some(saved) };
        saved.nav = nav;
        if let Ok(r) = postcard::from_bytes::<ViewRecord>(tail) {
            let p = &mut saved.persisted;
            (p.view, p.prefs, p.folders) = (r.view, r.prefs, FolderViews::from_entries(r.folders));
        }
        Some(saved)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let p = &self.persisted;
        let mut bytes = postcard::to_stdvec(&self.head()).ok()?;
        bytes.extend(postcard::to_stdvec(&self.nav).ok()?);
        bytes.extend(postcard::to_stdvec(&ViewRecord { view: p.view, prefs: p.prefs.clone(), folders: p.folders.entries() }).ok()?);
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
