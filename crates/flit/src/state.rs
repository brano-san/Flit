use flit_core::ui::Persisted;
use flit_core::Location;
use flit_platform::{Placement, Platform};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Saved {
    version: u32,
    pub persisted: Persisted,
    placement: Option<(i32, i32, i32, i32, bool)>,
}

impl Saved {
    pub fn new(persisted: Persisted, placement: Option<Placement>) -> Self {
        Self { version: VERSION, persisted, placement: placement.map(|p| (p.x, p.y, p.width, p.height, p.maximized)) }
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
        .and_then(|b| postcard::from_bytes::<Saved>(&b).ok())
        .filter(|s| s.version == VERSION)
        .unwrap_or_else(|| Saved::new(Persisted { path: Location::from_local(P::home()), ..Default::default() }, None))
}

pub fn save<P: Platform>(s: &Saved) {
    let Some(f) = file::<P>() else { return };
    let Ok(bytes) = postcard::to_stdvec(s) else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = f.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &f);
    }
}
