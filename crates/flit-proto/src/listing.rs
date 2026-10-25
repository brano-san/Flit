use crate::transport::ProtoError;
use flit_core::listing::attr;
use flit_core::{Listing, Meta};
use serde::{Deserialize, Serialize};

pub const WIRE_ATTRS: u32 = attr::DIR | attr::HIDDEN | attr::SYSTEM | attr::READONLY | attr::LINK;

pub fn check_name(name: &[u8], windows_names: bool) -> Result<(), ProtoError> {
    let forbidden = |b: u8| b == b'/' || b == 0 || (windows_names && (b == b'\\' || b == b':'));
    if name.is_empty() || name == b"." || name == b".." || name.iter().any(|&b| forbidden(b)) {
        return Err(ProtoError::Invalid("entry name is empty, a dot entry or contains a separator or NUL"));
    }
    Ok(())
}

pub fn check_meta(meta: &Meta) -> Result<(), ProtoError> {
    if meta.attrs & !WIRE_ATTRS != 0 {
        return Err(ProtoError::Invalid("entry carries attribute bits a peer may not set"));
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireEntry {
    pub name_len: u32,
    pub meta: Meta,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryBatch {
    pub names: Vec<u8>,
    pub entries: Vec<WireEntry>,
}

impl EntryBatch {
    pub fn push(&mut self, name: &[u8], meta: Meta) {
        self.names.extend_from_slice(name);
        self.entries.push(WireEntry { name_len: name.len() as u32, meta });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self, windows_names: bool) -> Result<impl Iterator<Item = (&[u8], Meta)> + '_, ProtoError> {
        let total: u64 = self.entries.iter().map(|e| e.name_len as u64).sum();
        if total != self.names.len() as u64 {
            return Err(ProtoError::Invalid("name lengths do not match the name buffer"));
        }
        let mut off = 0;
        for e in &self.entries {
            check_name(&self.names[off..off + e.name_len as usize], windows_names)?;
            check_meta(&e.meta)?;
            off += e.name_len as usize;
        }
        let mut off = 0;
        Ok(self.entries.iter().map(move |e| {
            let name = &self.names[off..off + e.name_len as usize];
            off += e.name_len as usize;
            (name, e.meta)
        }))
    }

    pub fn append_to(&self, l: &mut Listing, windows_names: bool) -> Result<(), ProtoError> {
        for (name, meta) in self.entries(windows_names)? {
            l.push(name, meta);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../test/listing.rs"]
mod tests;
