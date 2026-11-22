use crate::hash::{FxHashMap, FxHashSet};
use crate::listing::{Listing, attr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    AppleDouble,
    Full(&'a [u8]),
    Stem(&'a [u8]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Companion<'a> {
    pub owner: u32,
    pub sidecar: u32,
    pub kind: Kind<'a>,
}

fn extension(name: &[u8]) -> Option<(&[u8], &[u8])> {
    let dot = name.iter().rposition(|&b| b == b'.')?;
    (dot > 0 && dot + 1 < name.len()).then(|| name.split_at(dot))
}

fn sidecar(name: &[u8]) -> Option<(&[u8], Kind<'_>)> {
    if let Some(owner) = name.strip_prefix(b"._").filter(|p| !p.is_empty()) {
        return Some((owner, Kind::AppleDouble));
    }
    let (base, ext) = extension(name)?;
    (ext.eq_ignore_ascii_case(b".xmp") || ext.eq_ignore_ascii_case(b".aae")).then_some((base, Kind::Full(ext)))
}

impl Kind<'_> {
    pub fn beside(self, primary: &[u8]) -> Vec<u8> {
        match self {
            Kind::AppleDouble => [b"._", primary].concat(),
            Kind::Full(ext) => [primary, ext].concat(),
            Kind::Stem(ext) => [extension(primary).map_or(primary, |(base, _)| base), ext].concat(),
        }
    }
}

fn folded(name: &[u8]) -> Vec<u8> {
    std::str::from_utf8(name).map_or_else(|_| name.to_ascii_lowercase(), |u| u.to_lowercase().into_bytes())
}

pub fn associated(list: &Listing, case_sensitive: bool) -> Vec<Companion<'_>> {
    let eligible = |i: u32| list.entries[i as usize].attrs & (attr::DIR | attr::DELETED | attr::STREAM) == 0;
    let possible: Vec<_> = (0..list.len() as u32).filter(|&i| eligible(i))
        .filter_map(|i| Some((i, sidecar(list.name(i))?))).collect();
    if possible.is_empty() {
        return Vec::new();
    }
    let key = |i: u32, name: &[u8]| {
        let folder = list.folder(i);
        if case_sensitive { (folder.to_vec(), name.to_vec()) } else { (folded(folder), folded(name)) }
    };
    let mut names = FxHashMap::default();
    let mut stems = FxHashMap::default();
    for i in (0..list.len() as u32).filter(|&i| eligible(i)) {
        let name = list.name(i);
        if sidecar(name).is_some() {
            continue;
        }
        names.entry(key(i, name)).and_modify(|v| *v = None).or_insert(Some(i));
        if let Some((stem, _)) = extension(name) {
            stems.entry(key(i, stem)).and_modify(|v| *v = None).or_insert(Some(i));
        }
    }
    possible.into_iter().filter_map(|(sidecar, (base, kind))| {
        let key = key(sidecar, base);
        if let Some(owner) = names.get(&key) {
            Some(Companion { owner: (*owner)?, sidecar, kind })
        } else if let Kind::Full(ext) = kind {
            let owner = stems.get(&key).copied().flatten()?;
            Some(Companion { owner, sidecar, kind: Kind::Stem(ext) })
        } else {
            None
        }
    }).collect()
}

pub fn hidden(list: &Listing, case_sensitive: bool) -> FxHashSet<u32> {
    associated(list, case_sensitive).into_iter().map(|c| c.sidecar).collect()
}

#[cfg(test)]
#[path = "../test/companions.rs"]
mod tests;
