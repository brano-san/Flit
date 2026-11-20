use crate::path::PathKind;
use crate::{FsPath, Location};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub pinned: Vec<FsPath>,
    pub complete: bool,
}

pub trait QuickAccess {
    fn pinned(&mut self) -> Option<Snapshot>;
    fn stamp(&mut self) -> Option<(u64, u128)>;
    fn set_pinned(&mut self, folder: &FsPath, pin: bool) -> bool;
    fn is_dir(&mut self, path: &FsPath) -> bool;
}

impl<T: QuickAccess + ?Sized> QuickAccess for Box<T> {
    fn pinned(&mut self) -> Option<Snapshot> {
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

impl PinsDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
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

fn same(a: &FsPath, b: &FsPath) -> bool {
    a.same(b) || a.display().to_lowercase() == b.display().to_lowercase()
}

fn has(list: &[FsPath], p: &FsPath) -> bool {
    list.iter().any(|x| same(x, p))
}

fn listed(list: &[Location], loc: &Location) -> bool {
    match eligible(loc) {
        Some(p) => list.iter().filter_map(eligible).any(|x| same(&x, &p)),
        None => list.contains(loc),
    }
}

pub fn pins_diff(old: &[Location], new: &[Location]) -> PinsDiff {
    let mut diff = PinsDiff::default();
    for l in new {
        if !listed(old, l) && !listed(&diff.added, l) {
            diff.added.push(l.clone());
        }
    }
    for l in old {
        if !listed(new, l) && !listed(&diff.removed, l) {
            diff.removed.push(l.clone());
        }
    }
    diff
}

pub fn apply_incoming(flit: &[Location], change: &Change) -> Option<Vec<Location>> {
    let mut out: Vec<Location> = flit.iter().filter(|l| !eligible(l).is_some_and(|p| has(&change.removed, &p))).cloned().collect();
    for p in &change.added {
        let loc = Location::from_local(p.clone());
        if eligible(&loc).is_some() && !listed(&out, &loc) {
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

pub const HOLD_POLLS: u32 = 3;

pub struct QuickSync<Q: QuickAccess> {
    quick: Q,
    on: bool,
    known: Option<Vec<FsPath>>,
    stamp: Option<(u64, u128)>,
    held: Vec<(FsPath, bool, u32)>,
}

impl<Q: QuickAccess> QuickSync<Q> {
    pub fn new(quick: Q) -> Self {
        Self { quick, on: false, known: None, stamp: None, held: Vec::new() }
    }

    pub fn enabled(&self) -> bool {
        self.on
    }

    pub fn quick(&self) -> &Q {
        &self.quick
    }

    fn settle(&self, mut now: Vec<FsPath>) -> Vec<FsPath> {
        for (p, pin, _) in &self.held {
            if *pin && !has(&now, p) {
                now.push(p.clone());
            } else if !*pin {
                now.retain(|x| !same(x, p));
            }
        }
        now
    }

    fn reread(&mut self) -> Option<Vec<FsPath>> {
        self.stamp = self.quick.stamp();
        let snap = self.quick.pinned();
        if snap.is_none() {
            self.stamp = None;
        }
        snap.map(|s| self.settle(folders(&s.pinned)))
    }

    fn hold(&mut self, p: FsPath, pin: bool) {
        self.held.retain(|(x, _, _)| !same(x, &p));
        self.held.push((p, pin, HOLD_POLLS));
    }

    pub fn resume(&mut self) {
        self.on = true;
        self.known = self.reread();
    }

    pub fn enable(&mut self, flit: &[Location]) -> Change {
        self.on = true;
        let Some(quick) = self.reread() else {
            self.known = None;
            return Change::default();
        };
        for p in flit.iter().filter_map(eligible) {
            if !has(&quick, &p) && self.quick.is_dir(&p) {
                if self.quick.set_pinned(&p, true) {
                    self.hold(p, true);
                } else {
                    tracing::info!("cannot pin {} to Quick access", p.display());
                }
            }
        }
        let added: Vec<FsPath> = quick.iter().filter(|p| !flit.iter().filter_map(eligible).any(|f| same(&f, p))).cloned().collect();
        self.known = Some(self.reread().unwrap_or_else(|| self.settle(quick)));
        Change { added, removed: Vec::new() }
    }

    pub fn disable(&mut self) {
        self.on = false;
        self.known = None;
        self.stamp = None;
        self.held.clear();
    }

    pub fn flit_changed(&mut self, old: &[Location], new: &[Location]) {
        if !self.on {
            return;
        }
        let diff = pins_diff(old, new);
        let mut touched = false;
        for p in diff.added.iter().filter_map(eligible) {
            if !self.known.as_deref().is_some_and(|k| has(k, &p)) && self.quick.is_dir(&p) {
                touched = true;
                if self.quick.set_pinned(&p, true) {
                    self.hold(p.clone(), true);
                    if let Some(k) = &mut self.known {
                        k.push(p);
                    }
                }
            }
        }
        for p in diff.removed.iter().filter_map(eligible) {
            if self.known.as_deref().is_some_and(|k| has(k, &p)) && self.quick.is_dir(&p) {
                touched = true;
                if self.quick.set_pinned(&p, false) {
                    self.hold(p.clone(), false);
                    if let Some(k) = &mut self.known {
                        k.retain(|x| !same(x, &p));
                    }
                }
            }
        }
        if touched {
            if let Some(now) = self.reread() {
                self.known = Some(now);
            }
        }
    }

    pub fn poll(&mut self) -> Option<Change> {
        if !self.on {
            return None;
        }
        for h in &mut self.held {
            h.2 = h.2.saturating_sub(1);
        }
        self.held.retain(|h| h.2 > 0);
        let stamp = self.quick.stamp();
        if stamp.is_some() && stamp == self.stamp && self.known.is_some() {
            return None;
        }
        let Some(snap) = self.quick.pinned() else {
            self.stamp = None;
            return None;
        };
        self.stamp = stamp;
        let now = self.settle(folders(&snap.pinned));
        let Some(known) = self.known.take() else {
            self.known = Some(now);
            return None;
        };
        let added: Vec<FsPath> = now.iter().filter(|p| !has(&known, p)).cloned().collect();
        let missing: Vec<FsPath> = known.iter().filter(|p| !has(&now, p)).cloned().collect();
        let removed: Vec<FsPath> = missing.iter().filter(|p| self.quick.is_dir(p)).cloned().collect();
        let unavailable: Vec<FsPath> = missing.iter().filter(|p| !has(&removed, p)).cloned().collect();
        let reset = stamp.is_none() || (now.is_empty() && !known.is_empty()) || removed.len() > 1 || (known.len() > 2 && missing.len() * 2 > known.len());
        if reset {
            tracing::info!("Quick access lost {} of {} pinned folders at once; Flit keeps its bookmarks", missing.len(), known.len());
            self.known = Some(now.into_iter().chain(unavailable).collect());
            return None;
        }
        let (removed, kept) = if snap.complete { (removed, unavailable) } else { (Vec::new(), missing) };
        self.known = Some(now.into_iter().chain(kept).collect());
        let change = Change { added, removed };
        (!change.is_empty()).then_some(change)
    }
}

#[cfg(test)]
#[path = "../test/quick_sync.rs"]
mod tests;
