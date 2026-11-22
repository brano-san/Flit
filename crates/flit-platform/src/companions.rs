use crate::{Fs, Job};
use flit_core::companions::{self, Kind};
use flit_core::hash::{FxHashMap, FxHashSet};
use flit_core::listing::{Listing, ListingBuilder};
use flit_core::provider::FileOp;
use flit_core::sort::CaseMode;
use flit_core::FsPath;

fn listing(fs: &dyn Fs, dir: &FsPath, job: &Job) -> Result<Listing, String> {
    let mut batch = |_: &ListingBuilder<'_>| !job.cancelled();
    let mut b = ListingBuilder::new(if dir.case_sensitive() { CaseMode::Sensitive } else { CaseMode::Insensitive }, &mut batch);
    fs.read_dir(dir, &mut b).map_err(|e| e.to_string())?;
    if job.cancelled() { return Err(crate::CANCELLED.into()); }
    Ok(b.listing)
}

fn key(p: &FsPath) -> Vec<u8> {
    if p.case_sensitive() { return p.as_bytes().to_vec(); }
    std::str::from_utf8(p.as_bytes()).map_or_else(|_| p.as_bytes().to_ascii_lowercase(), |u| u.to_lowercase().into_bytes())
}

pub(crate) struct Sidecar {
    path: FsPath,
    rename: Rename,
}

enum Rename {
    AppleDouble,
    Full(Vec<u8>),
    Stem(Vec<u8>),
}

impl Rename {
    fn beside(&self, name: &[u8]) -> Vec<u8> {
        match self {
            Self::AppleDouble => Kind::AppleDouble,
            Self::Full(ext) => Kind::Full(ext),
            Self::Stem(ext) => Kind::Stem(ext),
        }.beside(name)
    }
}

fn read(fs: &dyn Fs, sources: &[&FsPath], job: &Job) -> Result<FxHashMap<Vec<u8>, Vec<Sidecar>>, String> {
    let mut directories = FxHashSet::default();
    let mut sides = FxHashMap::<_, Vec<_>>::default();
    let wanted: FxHashSet<_> = sources.iter().map(|p| key(p)).collect();
    for dir in sources.iter().filter_map(|p| p.parent()) {
        if !directories.insert(key(&dir)) { continue; }
        let l = listing(fs, &dir, job)?;
        for c in companions::associated(&l, dir.case_sensitive()) {
            let owner = key(&dir.join(l.name(c.owner)));
            if !wanted.contains(&owner) { continue; }
            let path = dir.join(l.name(c.sidecar));
            let rename = match c.kind {
                Kind::AppleDouble => Rename::AppleDouble,
                Kind::Full(ext) => Rename::Full(ext.to_vec()),
                Kind::Stem(ext) => Rename::Stem(ext.to_vec()),
            };
            sides.entry(owner).or_default().push(Sidecar { path, rename });
        }
    }
    Ok(sides)
}

pub(crate) type Groups = FxHashMap<Vec<u8>, Vec<Sidecar>>;

impl Job {
    pub fn prepare_companions(&self, fs: &dyn Fs, mut op: FileOp) -> Result<FileOp, String> {
        let sources: Vec<_> = match &op {
            FileOp::Copy { items, .. } | FileOp::Move { items, .. } => items.iter().collect(),
            FileOp::MoveTo(pairs) => pairs.iter().map(|(from, _)| from).collect(),
            FileOp::Rename { item, .. } => vec![item],
            _ => return Ok(op),
        };
        let groups = read(fs, &sources, self)?;
        if groups.is_empty() { return Ok(op); }
        let attached: FxHashSet<_> = groups.values().flatten().map(|side| key(&side.path)).collect();
        match &mut op {
            FileOp::Copy { items, .. } | FileOp::Move { items, .. } => items.retain(|path| !attached.contains(&key(path))),
            FileOp::MoveTo(pairs) => pairs.retain(|(from, _)| !attached.contains(&key(from))),
            FileOp::Rename { item, name } => {
                if let Some(dir) = item.parent() {
                    op = FileOp::MoveTo(vec![(item.clone(), dir.join(name))]);
                }
            }
            _ => {}
        }
        let _ = self.0.companions.set(groups);
        Ok(op)
    }

    pub fn has_companion_groups(&self) -> bool {
        self.0.companions.get().is_some()
    }

    pub fn has_companions(&self, source: &FsPath) -> bool {
        self.0.companions.get().is_some_and(|groups| groups.contains_key(&key(source)))
    }

    pub fn companion_targets(&self, source: &FsPath, target: &FsPath) -> Vec<(FsPath, FsPath)> {
        let Some(sides) = self.0.companions.get().and_then(|groups: &Groups| groups.get(&key(source))) else { return Vec::new(); };
        let Some(folder) = target.parent() else { return Vec::new(); };
        sides.iter().filter_map(|side| {
            let to = folder.join(&side.rename.beside(target.file_name()));
            (side.path != to).then(|| (side.path.clone(), to))
        }).collect()
    }

    pub fn companion_collision(&self, source: &FsPath, target: &FsPath) -> Option<String> {
        let targets = self.companion_targets(source, target);
        if targets.is_empty() { return None; }
        let mut planned = FxHashSet::default();
        let primary = key(target);
        for (from, to) in targets {
            let name = key(&to);
            if name == primary || name != key(&from) && std::fs::symlink_metadata(crate::std_path(&to)).is_ok()
                || !planned.insert(name) {
                return Some(companion_collision(&to));
            }
        }
        None
    }
}

pub fn companion_collision(path: &FsPath) -> String {
    flit_core::tr_en!(flit_core::i18n::S::CompanionsCollision, path = path.display())
}

#[cfg(test)]
#[path = "../test/companions.rs"]
mod tests;
