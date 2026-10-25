use crate::transport::ProtoError;
use flit_core::{Listing, Meta};
use serde::{Deserialize, Serialize};

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

    pub fn entries(&self) -> Result<impl Iterator<Item = (&[u8], Meta)> + '_, ProtoError> {
        let total: u64 = self.entries.iter().map(|e| e.name_len as u64).sum();
        if total != self.names.len() as u64 {
            return Err(ProtoError::Invalid("name lengths do not match the name buffer"));
        }
        let mut off = 0;
        let mut bad = false;
        for e in &self.entries {
            let name = &self.names[off..off + e.name_len as usize];
            off += e.name_len as usize;
            bad |= name.is_empty() || name == b"." || name == b".." || name.iter().any(|&b| b == b'/' || b == 0);
        }
        if bad {
            return Err(ProtoError::Invalid("entry name is empty, a dot entry or contains '/' or NUL"));
        }
        let mut off = 0;
        Ok(self.entries.iter().map(move |e| {
            let name = &self.names[off..off + e.name_len as usize];
            off += e.name_len as usize;
            (name, e.meta)
        }))
    }

    pub fn append_to(&self, l: &mut Listing) -> Result<(), ProtoError> {
        for (name, meta) in self.entries()? {
            l.push(name, meta);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_names_roundtrip_verbatim() {
        let mut b = EntryBatch::default();
        let names: [&[u8]; 5] = [b"plain", b"\xFF\xFEnot-utf8", "отчёт 🎉".as_bytes(), b"back\\slash:colon?", b"\x01ctl"];
        for (i, n) in names.iter().enumerate() {
            b.push(n, Meta { size: i as u64, ..Default::default() });
        }
        let wire = postcard::to_stdvec(&b).unwrap();
        let back: EntryBatch = postcard::from_bytes(&wire).unwrap();
        let mut l = Listing::default();
        back.append_to(&mut l).unwrap();
        for (i, n) in names.iter().enumerate() {
            assert_eq!(l.name(i as u32), *n);
            assert_eq!(l.entries[i].size, i as u64);
        }
    }

    #[test]
    fn hostile_batches_rejected() {
        for evil in [&b".."[..], b"a/b", b"", b"nul\0"] {
            let mut b = EntryBatch::default();
            b.push(b"ok", Meta::default());
            b.push(evil, Meta::default());
            assert!(b.entries().is_err(), "{evil:?}");
        }
        let mut b = EntryBatch::default();
        b.push(b"abc", Meta::default());
        b.entries[0].name_len = 99;
        assert!(b.entries().is_err());
        b.entries[0].name_len = u32::MAX;
        b.entries.push(WireEntry { name_len: 4, meta: Meta::default() });
        assert!(b.entries().is_err(), "length sum must not wrap");
    }
}
