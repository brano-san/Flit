use crate::path::PathKind;
use crate::{FsPath, Location};

pub trait QuickAccess {
    fn pinned(&mut self) -> Option<Vec<FsPath>>;
    fn stamp(&mut self) -> Option<(u64, u128)>;
    fn set_pinned(&mut self, folder: &FsPath, pin: bool) -> bool;
    fn is_dir(&mut self, path: &FsPath) -> bool;
}

impl<T: QuickAccess + ?Sized> QuickAccess for Box<T> {
    fn pinned(&mut self) -> Option<Vec<FsPath>> {
        (**self).pinned()
    }
    fn stamp(&mut self) -> Option<(u64, u128)> {
        (**self).stamp()
    }
    fn set_pinned(&mut self, folder: &FsPath, pin: bool) -> bool {
        (**self).set_pinned(folder, pin)
    }
    fn is_dir(&mut self, path: &FsPath) -> bool {
        (**self).is_dir(path)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PinsDiff {
    pub added: Vec<Location>,
    pub removed: Vec<Location>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub added: Vec<FsPath>,
    pub removed: Vec<FsPath>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

pub fn eligible(loc: &Location) -> Option<FsPath> {
    loc.to_windows().filter(|p| p.kind() == PathKind::Drive)
}

fn has(list: &[FsPath], p: &FsPath) -> bool {
    list.iter().any(|x| x.same(p))
}

fn holds(list: &[Location], loc: &Location) -> bool {
    match eligible(loc) {
        Some(p) => list.iter().filter_map(eligible).any(|x| x.same(&p)),
        None => list.contains(loc),
    }
}

pub fn pins_diff(old: &[Location], new: &[Location]) -> PinsDiff {
    let mut diff = PinsDiff::default();
    for l in new {
        if !holds(old, l) && !holds(&diff.added, l) {
            diff.added.push(l.clone());
        }
    }
    for l in old {
        if !holds(new, l) && !holds(&diff.removed, l) {
            diff.removed.push(l.clone());
        }
    }
    diff
}

pub fn apply_incoming(flit: &[Location], change: &Change) -> Option<Vec<Location>> {
    let mut out: Vec<Location> = flit.iter().filter(|l| !eligible(l).is_some_and(|p| has(&change.removed, &p))).cloned().collect();
    for p in &change.added {
        let loc = Location::from_local(p.clone());
        if eligible(&loc).is_some() && !holds(&out, &loc) {
            out.push(loc);
        }
    }
    (out != flit).then_some(out)
}

fn folders(list: &[FsPath]) -> Vec<FsPath> {
    let mut out: Vec<FsPath> = Vec::new();
    for p in list {
        if eligible(&Location::from_local(p.clone())).is_some() && !has(&out, p) {
            out.push(p.clone());
        }
    }
    out
}

pub struct QuickSync<Q: QuickAccess> {
    quick: Q,
    on: bool,
    known: Vec<FsPath>,
    stamp: Option<(u64, u128)>,
}

impl<Q: QuickAccess> QuickSync<Q> {
    pub fn new(quick: Q) -> Self {
        Self { quick, on: false, known: Vec::new(), stamp: None }
    }

    pub fn enabled(&self) -> bool {
        self.on
    }

    pub fn quick(&self) -> &Q {
        &self.quick
    }

    fn reread(&mut self) -> Option<Vec<FsPath>> {
        self.stamp = self.quick.stamp();
        self.quick.pinned().map(|p| folders(&p))
    }

    pub fn resume(&mut self) {
        self.on = true;
        self.known = self.reread().unwrap_or_default();
    }

    pub fn enable(&mut self, flit: &[Location]) -> Change {
        self.on = true;
        let Some(quick) = self.reread() else {
            self.known.clear();
            return Change::default();
        };
        for p in flit.iter().filter_map(eligible) {
            if !has(&quick, &p) && self.quick.is_dir(&p) && !self.quick.set_pinned(&p, true) {
                tracing::info!("cannot pin {} to Quick access", p.display());
            }
        }
        let added: Vec<FsPath> = quick.iter().filter(|p| !flit.iter().filter_map(eligible).any(|f| f.same(p))).cloned().collect();
        self.known = self.reread().unwrap_or(quick);
        Change { added, removed: Vec::new() }
    }

    pub fn disable(&mut self) {
        self.on = false;
        self.known.clear();
        self.stamp = None;
    }

    pub fn flit_changed(&mut self, old: &[Location], new: &[Location]) {
        if !self.on {
            return;
        }
        let diff = pins_diff(old, new);
        let mut touched = false;
        for p in diff.added.iter().filter_map(eligible) {
            if !has(&self.known, &p) && self.quick.is_dir(&p) {
                touched = true;
                if self.quick.set_pinned(&p, true) {
                    self.known.push(p);
                }
            }
        }
        for p in diff.removed.iter().filter_map(eligible) {
            if has(&self.known, &p) && self.quick.is_dir(&p) {
                touched = true;
                if self.quick.set_pinned(&p, false) {
                    self.known.retain(|k| !k.same(&p));
                }
            }
        }
        if touched {
            if let Some(now) = self.reread() {
                self.known = now;
            }
        }
    }

    pub fn poll(&mut self) -> Option<Change> {
        if !self.on {
            return None;
        }
        let stamp = self.quick.stamp();
        if stamp.is_some() && stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        let now = folders(&self.quick.pinned()?);
        let added: Vec<FsPath> = now.iter().filter(|p| !has(&self.known, p)).cloned().collect();
        let removed: Vec<FsPath> = self.known.iter().filter(|p| !has(&now, p)).cloned().collect();
        let removed: Vec<FsPath> = removed.into_iter().filter(|p| self.quick.is_dir(p)).collect();
        let gone: Vec<FsPath> = self.known.iter().filter(|p| !has(&now, p) && !has(&removed, p)).cloned().collect();
        self.known = now;
        self.known.extend(gone);
        let change = Change { added, removed };
        (!change.is_empty()).then_some(change)
    }
}

#[cfg(test)]
#[path = "../test/quick_sync.rs"]
mod tests;
