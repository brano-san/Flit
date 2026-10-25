use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const K: u64 = 0x517c_c1b7_2722_0a95;

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes(c.try_into().unwrap()));
        }
        for &b in chunks.remainder() {
            self.add(b as u64);
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i)
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64)
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuild>;

pub fn hash<T: Hash + ?Sized>(v: &T) -> u64 {
    let mut h = FxHasher::default();
    v.hash(&mut h);
    h.finish()
}
