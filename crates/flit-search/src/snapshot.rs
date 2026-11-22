use crate::index::{Checkpoint, IndexRecord, IndexSink, VolumeIndex, KEPT_ATTRS};
use flit_core::attr;
use std::io::{self, Write};

const MAGIC: &[u8; 8] = b"FLITIDX\0";
const VERSION: u32 = 2;
const RECORD_FIXED: usize = 8 + 8 + 8 + 8 + 4 + 2;

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

const K: u64 = 0x9e37_79b9_7f4a_7c15;
const LANES: [u64; 4] = [K, K ^ 1, K ^ 2, K ^ 3];
const CHUNK: usize = 1 << 20;

fn mix_blocks(lanes: &mut [u64; 4], bytes: &[u8]) {
    for block in bytes.chunks_exact(32) {
        for (lane, chunk) in lanes.iter_mut().zip(block.chunks_exact(8)) {
            let mut word = [0; 8];
            word.copy_from_slice(chunk);
            let w = u64::from_le_bytes(word);
            *lane = (*lane ^ w).wrapping_mul(K).rotate_left(29);
        }
    }
}

fn fold_lanes(lanes: &[u64; 4], remainder: &[u8], len: u64) -> u64 {
    lanes.iter().fold(fnv1a(remainder) ^ len, |h, &l| (h ^ l).wrapping_mul(K).rotate_left(29))
}

fn words(bytes: &[u8]) -> u64 {
    let mut lanes = LANES;
    let whole = bytes.len() / 32 * 32;
    mix_blocks(&mut lanes, &bytes[..whole]);
    fold_lanes(&lanes, &bytes[whole..], bytes.len() as u64)
}

struct Summed<W: Write> {
    w: W,
    buf: Vec<u8>,
    lanes: [u64; 4],
    len: u64,
}

impl<W: Write> Summed<W> {
    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() >= CHUNK {
            self.drain()?;
        }
        Ok(())
    }

    fn drain(&mut self) -> io::Result<()> {
        let whole = self.buf.len() / 32 * 32;
        mix_blocks(&mut self.lanes, &self.buf[..whole]);
        self.w.write_all(&self.buf[..whole])?;
        self.len += whole as u64;
        self.buf.drain(..whole);
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        self.drain()?;
        let sum = fold_lanes(&self.lanes, &self.buf, self.len + self.buf.len() as u64);
        self.w.write_all(&self.buf)?;
        self.w.write_all(&sum.to_le_bytes())?;
        Ok(self.w)
    }
}

fn checksum(version: u32, body: &[u8]) -> u64 {
    if version == 1 {
        fnv1a(body)
    } else {
        words(body)
    }
}

pub fn file_name(volume: &str) -> String {
    format!("{:016x}.idx", fnv1a(volume.as_bytes()))
}

fn put_bytes<W: Write>(out: &mut Summed<W>, bytes: &[u8]) -> io::Result<()> {
    out.put(&(bytes.len() as u16).to_le_bytes())?;
    out.put(bytes)
}

pub fn save(v: &VolumeIndex, cp: Checkpoint, out: &mut Vec<u8>) {
    out.clear();
    let _ = save_to(v, cp, out);
}

pub fn save_to<W: Write>(v: &VolumeIndex, cp: Checkpoint, w: W) -> io::Result<W> {
    let mut out = Summed { w, buf: Vec::with_capacity(CHUNK + RECORD_FIXED + 2 + u16::MAX as usize), lanes: LANES, len: 0 };
    out.put(MAGIC)?;
    out.put(&VERSION.to_le_bytes())?;
    out.put(&cp.journal.to_le_bytes())?;
    out.put(&cp.next.to_le_bytes())?;
    put_bytes(&mut out, v.id.as_bytes())?;
    put_bytes(&mut out, &v.root)?;
    out.put(&[v.sep])?;
    let alive = (0..v.len() as u32).filter(|&i| v.attrs[i as usize] & attr::DELETED == 0);
    out.put(&(alive.clone().count() as u64).to_le_bytes())?;
    for i in alive {
        let k = i as usize;
        out.put(&v.ids[k].to_le_bytes())?;
        out.put(&v.parent_id(i).to_le_bytes())?;
        out.put(&v.sizes[k].to_le_bytes())?;
        out.put(&v.modified[k].to_le_bytes())?;
        out.put(&(v.attrs[k] & KEPT_ATTRS).to_le_bytes())?;
        put_bytes(&mut out, v.name(i))?;
    }
    out.finish()
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let (s, rest) = self.rest.split_at_checked(n).ok_or("the snapshot is truncated")?;
        self.rest = rest;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let (a, rest) = self.rest.split_first_chunk().ok_or("the snapshot is truncated")?;
        self.rest = rest;
        Ok(*a)
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = u16::from_le_bytes(self.array()?) as usize;
        self.take(n)
    }
}

pub fn load(bytes: &[u8]) -> Result<(VolumeIndex, Checkpoint), String> {
    load_with(bytes, true)
}

fn load_with(bytes: &[u8], bulk: bool) -> Result<(VolumeIndex, Checkpoint), String> {
    let (body, sum) = bytes.split_last_chunk().ok_or("the snapshot is truncated")?;
    let (magic, rest) = body.split_first_chunk().ok_or("the snapshot is truncated")?;
    if magic != MAGIC {
        return Err("not a Flit index snapshot".into());
    }
    let mut r = Reader { rest };
    let version = r.u32()?;
    if version != VERSION && version != 1 {
        return Err(format!("snapshot version {version} is not supported"));
    }
    if u64::from_le_bytes(*sum) != checksum(version, body) {
        return Err("the snapshot checksum does not match".into());
    }
    let cp = Checkpoint { journal: r.u64()?, next: r.u64()? as i64 };
    let id = String::from_utf8(r.bytes()?.to_vec()).map_err(|_| "the snapshot volume id is not UTF-8")?;
    let root = r.bytes()?.to_vec();
    let [sep] = r.array()?;
    let count = r.u64()?;
    if count > (body.len() / RECORD_FIXED) as u64 {
        return Err("the snapshot record count is impossible".into());
    }
    let mut records = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (id, parent, size, modified, attrs) = (r.u64()?, r.u64()?, r.u64()?, r.u64()? as i64, r.u32()? & KEPT_ATTRS);
        let name = r.bytes()?;
        if name.is_empty() {
            return Err("the snapshot has an empty name".into());
        }
        records.push(IndexRecord { id, parent, name, size, modified, attrs });
    }
    if !r.rest.is_empty() {
        return Err("the snapshot has trailing bytes".into());
    }
    let mut v = VolumeIndex::new(&id, &root, sep);
    if !(bulk && v.load_fresh(&records)) {
        v.reserve(records.len(), body.len().saturating_sub(records.len() * RECORD_FIXED));
        records.iter().for_each(|r| v.upsert(r));
    }
    drop(records);
    v.commit();
    v.shrink();
    Ok((v, cp))
}

#[cfg(test)]
#[path = "../test/snapshot.rs"]
mod tests;
