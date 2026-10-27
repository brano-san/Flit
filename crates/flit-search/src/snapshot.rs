use crate::index::{Checkpoint, IndexRecord, IndexSink, VolumeIndex, KEPT_ATTRS};
use flit_core::attr;

const MAGIC: &[u8; 8] = b"FLITIDX\0";
const VERSION: u32 = 2;
const RECORD_FIXED: usize = 8 + 8 + 8 + 8 + 4 + 2;

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

fn words(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut lanes = [K, K ^ 1, K ^ 2, K ^ 3];
    let mut blocks = bytes.chunks_exact(32);
    for b in &mut blocks {
        for (k, lane) in lanes.iter_mut().enumerate() {
            let w = u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
            *lane = (*lane ^ w).wrapping_mul(K).rotate_left(29);
        }
    }
    lanes.iter().fold(fnv1a(blocks.remainder()) ^ bytes.len() as u64, |h, &l| (h ^ l).wrapping_mul(K).rotate_left(29))
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

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
}

pub fn save(v: &VolumeIndex, cp: Checkpoint, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&cp.journal.to_le_bytes());
    out.extend_from_slice(&cp.next.to_le_bytes());
    put_bytes(out, v.id.as_bytes());
    put_bytes(out, &v.root);
    out.push(v.sep);
    let alive: Vec<u32> = (0..v.len() as u32).filter(|&i| v.attrs[i as usize] & attr::DELETED == 0).collect();
    out.extend_from_slice(&(alive.len() as u64).to_le_bytes());
    out.reserve(alive.len() * (RECORD_FIXED + 16));
    for i in alive {
        let k = i as usize;
        out.extend_from_slice(&v.ids[k].to_le_bytes());
        out.extend_from_slice(&v.parent_id(i).to_le_bytes());
        out.extend_from_slice(&v.sizes[k].to_le_bytes());
        out.extend_from_slice(&v.modified[k].to_le_bytes());
        out.extend_from_slice(&(v.attrs[k] & KEPT_ATTRS).to_le_bytes());
        put_bytes(out, v.name(i));
    }
    let sum = checksum(VERSION, out);
    out.extend_from_slice(&sum.to_le_bytes());
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or("the snapshot is truncated")?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
        self.take(n)
    }
}

pub fn load(bytes: &[u8]) -> Result<(VolumeIndex, Checkpoint), String> {
    if bytes.len() < MAGIC.len() + 8 {
        return Err("the snapshot is truncated".into());
    }
    let (body, sum) = bytes.split_at(bytes.len() - 8);
    if body.get(..8) != Some(MAGIC) {
        return Err("not a Flit index snapshot".into());
    }
    let mut r = Reader { buf: body, at: 8 };
    let version = r.u32()?;
    if version != VERSION && version != 1 {
        return Err(format!("snapshot version {version} is not supported"));
    }
    if u64::from_le_bytes(sum.try_into().unwrap()) != checksum(version, body) {
        return Err("the snapshot checksum does not match".into());
    }
    let cp = Checkpoint { journal: r.u64()?, next: r.u64()? as i64 };
    let id = String::from_utf8(r.bytes()?.to_vec()).map_err(|_| "the snapshot volume id is not UTF-8")?;
    let root = r.bytes()?.to_vec();
    let sep = r.take(1)?[0];
    let count = r.u64()?;
    if count > (body.len() / RECORD_FIXED) as u64 {
        return Err("the snapshot record count is impossible".into());
    }
    let mut v = VolumeIndex::new(&id, &root, sep);
    v.reserve(count as usize, body.len().saturating_sub(r.at + count as usize * RECORD_FIXED));
    for _ in 0..count {
        let (id, parent, size, modified, attrs) = (r.u64()?, r.u64()?, r.u64()?, r.u64()? as i64, r.u32()? & KEPT_ATTRS);
        let name = r.bytes()?;
        if name.is_empty() {
            return Err("the snapshot has an empty name".into());
        }
        v.upsert(&IndexRecord { id, parent, name, size, modified, attrs });
    }
    if r.at != body.len() {
        return Err("the snapshot has trailing bytes".into());
    }
    v.commit();
    v.shrink();
    Ok((v, cp))
}

#[cfg(test)]
#[path = "../test/snapshot.rs"]
mod tests;
