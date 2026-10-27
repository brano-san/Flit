use super::*;
use crate::index::Order;

fn sample() -> VolumeIndex {
    let mut v = VolumeIndex::new("C:", br"C:\", b'\\');
    v.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    v.upsert(&IndexRecord { id: 10, parent: 5, name: "Документы".as_bytes(), size: 0, modified: 3, attrs: attr::DIR | attr::HIDDEN });
    v.upsert(&IndexRecord { id: 11, parent: 10, name: b"report.pdf", size: 12345, modified: 7, attrs: attr::READONLY });
    v.upsert(&IndexRecord { id: 12, parent: 5, name: b"gone.txt", size: 1, modified: 1, attrs: 0 });
    v.upsert(&IndexRecord { id: 13, parent: 99, name: b"orphan.txt", size: 2, modified: 2, attrs: 0 });
    v.commit();
    v.remove(12);
    v.commit();
    v
}

fn rows(v: &VolumeIndex) -> Vec<(String, u64, i64, u32)> {
    let mut v = v.clone();
    v.build_order(Order::Path);
    v.perms[Order::Path as usize]
        .iter()
        .map(|&i| {
            let mut p = Vec::new();
            v.path_into(i, false, &mut Vec::new(), &mut p);
            (String::from_utf8(p).unwrap(), v.sizes[i as usize], v.modified[i as usize], v.attrs[i as usize] & KEPT_ATTRS)
        })
        .collect()
}

#[test]
fn snapshots_round_trip_with_their_checkpoint() {
    let v = sample();
    let cp = Checkpoint { journal: 0x01D9_0000_1234, next: 987_654 };
    let mut buf = Vec::new();
    save(&v, cp, &mut buf);
    let (back, got) = load(&buf).unwrap();
    assert_eq!(got, cp);
    assert_eq!((back.id.as_str(), back.root.as_slice(), back.sep), ("C:", br"C:\".as_slice(), b'\\'));
    assert_eq!(back.live(), v.live());
    assert_eq!(rows(&back), rows(&v));
    assert!(back.lookup(12).is_none());
    assert!(rows(&back).iter().any(|r| r.0 == r"C:\Документы\report.pdf" && r.1 == 12345));
    assert_eq!(file_name("C:"), file_name("C:"));
    assert_ne!(file_name("C:"), file_name("D:"));
    assert!(file_name(r"D:\some\walk root").ends_with(".idx"));
}

#[test]
fn damaged_snapshots_are_refused() {
    let mut buf = Vec::new();
    save(&sample(), Checkpoint::default(), &mut buf);
    let mut flipped = buf.clone();
    flipped[40] ^= 1;
    assert!(load(&flipped).err().unwrap().contains("checksum"));
    assert!(load(&buf[..buf.len() - 1]).is_err());
    assert!(load(&buf[..10]).is_err());
    assert!(load(b"").is_err());
    let mut foreign = buf.clone();
    foreign[0] = b'X';
    assert!(load(&foreign).err().unwrap().contains("not a Flit"));
    let mut lying = Vec::new();
    lying.extend_from_slice(MAGIC);
    lying.extend_from_slice(&VERSION.to_le_bytes());
    lying.extend_from_slice(&[0; 16]);
    for field in [&b"C:"[..], br"C:\"] {
        lying.extend_from_slice(&(field.len() as u16).to_le_bytes());
        lying.extend_from_slice(field);
    }
    lying.push(b'\\');
    lying.extend_from_slice(&u64::MAX.to_le_bytes());
    let sum = checksum(VERSION, &lying);
    lying.extend_from_slice(&sum.to_le_bytes());
    assert!(load(&lying).err().unwrap().contains("impossible"));
}

#[test]
fn version_one_snapshots_with_the_old_checksum_still_load() {
    let mut buf = Vec::new();
    save(&sample(), Checkpoint { journal: 7, next: 9 }, &mut buf);
    let mut old = buf[..buf.len() - 8].to_vec();
    old[8..12].copy_from_slice(&1u32.to_le_bytes());
    let sum = fnv1a(&old);
    old.extend_from_slice(&sum.to_le_bytes());
    let (v, cp) = load(&old).unwrap();
    assert_eq!(cp, Checkpoint { journal: 7, next: 9 });
    assert_eq!(rows(&v), rows(&load(&buf).unwrap().0));
    old[20] ^= 1;
    assert!(load(&old).err().unwrap().contains("checksum"));
}

#[test]
fn snapshots_larger_than_one_write_chunk_stream_with_the_same_checksum() {
    let mut v = sample();
    for k in 0..60_000u64 {
        let name = format!("Отчёт {k} {}.txt", "x".repeat((k % 50) as usize));
        v.upsert(&IndexRecord { id: 1000 + k, parent: 10, name: name.as_bytes(), size: k, modified: k as i64, attrs: 0 });
    }
    v.commit();
    let mut buf = Vec::new();
    save(&v, Checkpoint { journal: 3, next: 4 }, &mut buf);
    assert!(buf.len() > 3 << 20);
    let body = &buf[..buf.len() - 8];
    assert_eq!(buf[buf.len() - 8..], checksum(VERSION, body).to_le_bytes());
    let (back, cp) = load(&buf).unwrap();
    assert_eq!((cp.journal, cp.next), (3, 4));
    assert_eq!(rows(&back), rows(&v));
}
