use flit_core::ui::{NavState, Persisted};
use flit_core::Location;
use flit_platform::{Placement, Platform};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const VERSION: u32 = 2;
const MIN_COL_W: f32 = 24.0;
const MAX_COL_W: f32 = 4000.0;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Saved {
    version: u32,
    pub persisted: Persisted,
    placement: Option<(i32, i32, i32, i32, bool)>,
    #[serde(skip)]
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
        let defaults = Persisted::default().col_w;
        for (w, d) in self.persisted.col_w.iter_mut().zip(defaults) {
            if !(w.is_finite() && (MIN_COL_W..=MAX_COL_W).contains(w)) {
                *w = d;
            }
        }
        self.placement = self.placement.filter(|&(_, _, w, h, _)| w > 0 && h > 0);
        self.nav = self.nav.sanitized();
        self
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (mut saved, tail) = postcard::take_from_bytes::<Saved>(bytes).ok()?;
        saved.nav = postcard::from_bytes(tail).unwrap_or_default();
        Some(saved)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let mut bytes = postcard::to_stdvec(self).ok()?;
        bytes.extend(postcard::to_stdvec(&self.nav).ok()?);
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
