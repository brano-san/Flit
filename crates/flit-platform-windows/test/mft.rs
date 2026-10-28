use super::*;
use flit_search::index::VolumeIndex;

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

fn utf16(name: &str) -> Vec<u8> {
    name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
}

fn v2(frn: u64, parent: u64, reason: u32, attrs: u32, name: &str) -> Vec<u8> {
    type R = USN_RECORD_V2;
    let wide = utf16(name);
    let at = offset_of!(R, FileName);
    let len = (at + wide.len()).div_ceil(8) * 8;
    let mut r = vec![0u8; len];
    put(&mut r, offset_of!(R, RecordLength), &(len as u32).to_le_bytes());
    put(&mut r, offset_of!(R, MajorVersion), &2u16.to_le_bytes());
    put(&mut r, offset_of!(R, FileReferenceNumber), &frn.to_le_bytes());
    put(&mut r, offset_of!(R, ParentFileReferenceNumber), &parent.to_le_bytes());
    put(&mut r, offset_of!(R, Usn), &77i64.to_le_bytes());
    put(&mut r, offset_of!(R, TimeStamp), &1234i64.to_le_bytes());
    put(&mut r, offset_of!(R, Reason), &reason.to_le_bytes());
    put(&mut r, offset_of!(R, FileAttributes), &attrs.to_le_bytes());
    put(&mut r, offset_of!(R, FileNameLength), &(wide.len() as u16).to_le_bytes());
    put(&mut r, offset_of!(R, FileNameOffset), &(at as u16).to_le_bytes());
    put(&mut r, at, &wide);
    r
}

fn v3(frn: u64, high: u64, parent: u64, reason: u32, name: &str) -> Vec<u8> {
    type R = USN_RECORD_V3;
    let wide = utf16(name);
    let at = offset_of!(R, FileName);
    let len = (at + wide.len()).div_ceil(8) * 8;
    let mut r = vec![0u8; len];
    put(&mut r, offset_of!(R, RecordLength), &(len as u32).to_le_bytes());
    put(&mut r, offset_of!(R, MajorVersion), &3u16.to_le_bytes());
    put(&mut r, offset_of!(R, FileReferenceNumber), &frn.to_le_bytes());
    put(&mut r, offset_of!(R, FileReferenceNumber) + 8, &high.to_le_bytes());
    put(&mut r, offset_of!(R, ParentFileReferenceNumber), &parent.to_le_bytes());
    put(&mut r, offset_of!(R, Usn), &88i64.to_le_bytes());
    put(&mut r, offset_of!(R, Reason), &reason.to_le_bytes());
    put(&mut r, offset_of!(R, FileAttributes), &FILE_ATTRIBUTE_DIRECTORY.0.to_le_bytes());
    put(&mut r, offset_of!(R, FileNameLength), &(wide.len() as u16).to_le_bytes());
    put(&mut r, offset_of!(R, FileNameOffset), &(at as u16).to_le_bytes());
    put(&mut r, at, &wide);
    r
}

fn text(r: &UsnRecord<'_>) -> String {
    let (mut wide, mut out) = (Vec::new(), Vec::new());
    r.name_into(&mut wide, &mut out);
    String::from_utf8(out).unwrap()
}

#[test]
fn usn_v2_and_v3_records_parse_and_damage_stops_the_walk() {
    assert_eq!((offset_of!(USN_RECORD_V2, FileName), offset_of!(USN_RECORD_V3, FileName)), (60, 76));
    let mut buf = v2(0x0002_0000_0000_0040, 0x0005_0000_0000_0005, USN_REASON_FILE_CREATE, FILE_ATTRIBUTE_ARCHIVE.0, "отчёт.txt");
    buf.extend(v3(0x0001_0000_0000_0041, 0, 0x0005_0000_0000_0005, USN_REASON_RENAME_NEW_NAME, "dir"));
    let mut v4 = vec![0u8; 16];
    put(&mut v4, 0, &16u32.to_le_bytes());
    put(&mut v4, 4, &4u16.to_le_bytes());
    buf.extend(v4);
    buf.extend(v3(7, 1, 5, 0, "refs"));
    buf.extend(v2(0x42, 0x41, USN_REASON_FILE_DELETE, 0, "x"));
    let got: Vec<UsnRecord<'_>> = usn_records(&buf).collect();
    assert_eq!(got.len(), 3, "the range-tracking record and the 128-bit id are skipped");
    assert_eq!((got[0].frn, got[0].parent, got[0].usn, got[0].time, got[0].reason), (0x0002_0000_0000_0040, 0x0005_0000_0000_0005, 77, 1234, USN_REASON_FILE_CREATE));
    assert_eq!(text(&got[0]), "отчёт.txt");
    assert!(got[1].is_dir() && !got[0].is_dir());
    assert_eq!((got[1].frn, got[1].usn, text(&got[1])), (0x0001_0000_0000_0041, 88, "dir".to_string()));
    assert_eq!((got[2].frn, got[2].parent), (0x42, 0x41));
    let first = v2(1, 5, 0, 0, "отчёт.txt").len();
    assert_eq!(usn_records(&buf[..first + 10]).count(), 1, "a truncated record ends the batch");
    let mut lying = v2(1, 5, 0, 0, "abc");
    put(&mut lying, offset_of!(USN_RECORD_V2, FileNameLength), &500u16.to_le_bytes());
    assert_eq!(usn_records(&lying).count(), 0);
    assert_eq!(usn_records(&[0u8; 16]).count(), 0);
}

const ROOT: u64 = 0x0005_0000_0000_0005;

#[test]
fn metafiles_and_their_subtrees_stay_out() {
    let mut h = Hidden::new(ROOT);
    let extend = 0x0001_0000_0000_000B;
    let rm = 0x0001_0000_0000_0040;
    assert!(!h.admit(ROOT, ROOT, true));
    assert!(!h.admit(extend, ROOT, true));
    assert!(!h.admit(rm, extend, true));
    assert!(!h.admit(0x0001_0000_0000_0041, rm, false));
    assert!(!h.admit(0x0001_0000_0000_0002, ROOT, false), "$LogFile");
    assert!(h.admit(0x0001_0000_0000_0050, ROOT, true));
    assert!(h.admit(0x0001_0000_0000_0051, 0x0001_0000_0000_0050, false));
}

#[test]
fn coalescing_keeps_the_final_state_of_each_file() {
    let mut buf = v2(100, ROOT, USN_REASON_FILE_CREATE, 0, "a.txt");
    buf.extend(v2(101, ROOT, USN_REASON_RENAME_OLD_NAME, 0, "old"));
    buf.extend(v2(100, ROOT, USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND | USN_REASON_CLOSE, 0, "a.txt"));
    buf.extend(v2(101, 0x50, USN_REASON_RENAME_NEW_NAME, 0, "new"));
    buf.extend(v2(102, ROOT, USN_REASON_RENAME_OLD_NAME, 0, "half"));
    buf.extend(v2(103, ROOT, USN_REASON_FILE_CREATE, 0, "tmp"));
    buf.extend(v2(103, ROOT, USN_REASON_FILE_CREATE | USN_REASON_FILE_DELETE | USN_REASON_CLOSE, 0, "tmp"));
    buf.extend(v2(104, 0x0001_0000_0000_000B, USN_REASON_DATA_EXTEND, 0, "$J"));
    buf.extend(v2(3, ROOT, USN_REASON_DATA_EXTEND, 0, "$Volume"));
    let mut hidden = Hidden::new(ROOT);
    let mut c = Coalesce::default();
    for r in usn_records(&buf) {
        c.push(&r, &mut hidden);
    }
    let got: Vec<Change> = c.finish().collect();
    assert_eq!(
        got,
        [
            Change::Upsert { frn: 100, parent: ROOT, name: b"a.txt".to_vec(), attrs: 0, time: 1234 },
            Change::Upsert { frn: 101, parent: 0x50, name: b"new".to_vec(), attrs: 0, time: 1234 },
            Change::Remove(103),
        ]
    );
}

const CHECK: [u8; 2] = [0x07, 0x00];

fn resident(ty: u32, name_len: u8, value: &[u8]) -> Vec<u8> {
    let at = 0x18;
    let len = (at + value.len()).div_ceil(8) * 8;
    let mut a = vec![0u8; len];
    put(&mut a, 0, &ty.to_le_bytes());
    put(&mut a, 4, &(len as u32).to_le_bytes());
    a[9] = name_len;
    put(&mut a, 16, &(value.len() as u32).to_le_bytes());
    put(&mut a, 20, &(at as u16).to_le_bytes());
    put(&mut a, at, value);
    a
}

fn nonresident(ty: u32, lowest_vcn: i64, data_size: u64, runlist: &[u8]) -> Vec<u8> {
    let at = 0x40;
    let len = (at + runlist.len()).div_ceil(8) * 8;
    let mut a = vec![0u8; len];
    put(&mut a, 0, &ty.to_le_bytes());
    put(&mut a, 4, &(len as u32).to_le_bytes());
    a[8] = 1;
    put(&mut a, 16, &lowest_vcn.to_le_bytes());
    put(&mut a, 32, &(at as u16).to_le_bytes());
    put(&mut a, 48, &data_size.to_le_bytes());
    put(&mut a, at, runlist);
    a
}

fn standard_information(modified: i64) -> Vec<u8> {
    let mut v = vec![0u8; 72];
    put(&mut v, 8, &modified.to_le_bytes());
    resident(ATTR_STANDARD_INFORMATION, 0, &v)
}

fn file_record(seq: u16, flags: u16, base: u64, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut r = vec![0u8; 1024];
    put(&mut r, 0, b"FILE");
    put(&mut r, 4, &0x30u16.to_le_bytes());
    put(&mut r, 6, &3u16.to_le_bytes());
    put(&mut r, 16, &seq.to_le_bytes());
    put(&mut r, 20, &0x38u16.to_le_bytes());
    put(&mut r, 22, &flags.to_le_bytes());
    put(&mut r, 32, &base.to_le_bytes());
    let mut at = 0x38;
    for a in attrs {
        put(&mut r, at, a);
        at += a.len();
    }
    put(&mut r, at, &ATTR_END.to_le_bytes());
    at += 8;
    put(&mut r, 24, &(at as u32).to_le_bytes());
    put(&mut r, 28, &1024u32.to_le_bytes());
    put(&mut r, 0x30, &CHECK);
    for i in 1..3 {
        let end = i * 512 - 2;
        let tail = [r[end], r[end + 1]];
        put(&mut r, 0x30 + 2 * i, &tail);
        put(&mut r, end, &CHECK);
    }
    r
}

#[test]
fn fixups_restore_sector_tails_and_reject_torn_records() {
    let mut r = file_record(4, RECORD_IN_USE, 0, &[standard_information(123), resident(ATTR_DATA, 0, &[0xAB; 600])]);
    assert_eq!(&r[510..512], &CHECK);
    let mut torn = r.clone();
    assert!(fixup(&mut r));
    assert_eq!(&r[510..512], &[0xAB, 0xAB]);
    assert_eq!(parse_record(&r), Some(RecordMeta { sequence: 4, base: 0, size: Some(600), modified: Some(123), reparse_tag: 0 }));
    torn[1022] ^= 0xFF;
    assert!(!fixup(&mut torn));
    let mut short = vec![0u8; 100];
    put(&mut short, 6, &3u16.to_le_bytes());
    assert!(!fixup(&mut short));
}

#[test]
fn records_yield_sizes_reparse_tags_and_extension_links() {
    let tag = 0xA000_000Cu32;
    let mut reparse = vec![0u8; 16];
    put(&mut reparse, 0, &tag.to_le_bytes());
    let attrs = [standard_information(9), resident(ATTR_DATA, 4, &[1; 9]), nonresident(ATTR_DATA, 0, 1 << 40, &[0x11, 0x01, 0x20, 0]), resident(ATTR_REPARSE_POINT, 0, &reparse)];
    let mut r = file_record(2, RECORD_IN_USE, 0, &attrs);
    assert!(fixup(&mut r));
    let m = parse_record(&r).unwrap();
    assert_eq!((m.size, m.modified, m.reparse_tag), (Some(1 << 40), Some(9), tag), "named streams do not count");
    let mut ext = file_record(1, RECORD_IN_USE, 0x0003_0000_0000_002A, &[nonresident(ATTR_DATA, 0, 77, &[0x11, 0x01, 0x20, 0])]);
    assert!(fixup(&mut ext));
    assert_eq!(parse_record(&ext).map(|m| (m.base, m.size, m.modified)), Some((42, Some(77), None)));
    let mut tail = file_record(1, RECORD_IN_USE, 0x2A, &[nonresident(ATTR_DATA, 16, 77, &[0x11, 0x01, 0x20, 0])]);
    assert!(fixup(&mut tail));
    assert_eq!(parse_record(&tail).unwrap().size, None, "only the first extent carries the size");
    let mut free = file_record(1, 0, 0, &[standard_information(1)]);
    assert!(fixup(&mut free));
    assert_eq!(parse_record(&free), None);
    let mut bad = file_record(1, RECORD_IN_USE, 0, &[]);
    put(&mut bad, 0, b"BAAD");
    assert_eq!(parse_record(&bad), None);
}

#[test]
fn data_runs_decode_relative_offsets_and_sparse_runs() {
    let list = [0x21, 0x08, 0x00, 0x01, 0x11, 0x04, 0xF0, 0x01, 0x02, 0x31, 0x10, 0x00, 0x00, 0x01, 0x00];
    assert_eq!(runs(&list), Some(vec![(Some(256), 8), (Some(240), 4), (None, 2), (Some(240 + 65536), 16)]));
    assert_eq!(runs(&[0x21, 0x08]), None, "truncated");
    assert_eq!(runs(&[0x10, 0x08, 0x00]), None, "zero-length size field");
    assert_eq!(runs(&[0x11, 0x01, 0x80, 0x00]), None, "a run before the start of the volume");
    let mut mft = file_record(1, RECORD_IN_USE, 0, &[standard_information(1), nonresident(ATTR_DATA, 0, 1 << 20, &list)]);
    assert!(fixup(&mut mft));
    assert_eq!(data_runs(&mft).and_then(runs).map(|r| r.len()), Some(4));
}

#[test]
fn a_real_volume_is_scanned_when_permitted() {
    let Some(root) = fixed_drives().into_iter().find(|r| describe(r).file_system == "NTFS") else {
        eprintln!("no NTFS volume, MFT test skipped");
        return;
    };
    let mut n = match open_ntfs(&root) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("MFT access needs administrator rights ({e}), MFT test skipped");
            return;
        }
    };
    let id = volume_id(&root);
    let mut v = VolumeIndex::new(&id, root.display().as_bytes(), b'\\');
    let (journal, meta) = n.prepare(&mut v).unwrap();
    let cp = n.emit(journal, &meta, &mut v).unwrap();
    v.commit();
    assert!(v.live() > 1000, "{} entries", v.live());
    assert!(meta.iter().filter(|m| m.known && m.sized && m.size > 0).count() > 100);
    assert!(matches!(n.changes(cp, &mut v), Ok(Some(next)) if next.next >= cp.next));
}

#[test]
fn volumes_without_mft_access_are_walked_and_never_resumed_blindly() {
    let dir = crate::test_temp().join(format!("flit-mft-walk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("f.txt"), b"abc").unwrap();
    let rs = dir.to_str().unwrap().to_owned();
    let root = FsPath::parse(&rs).unwrap();
    let mut fresh = MftIndex::with_roots(vec![root.clone()]);
    let volume = fresh.volumes().unwrap()[0].id.clone();
    let mut v = VolumeIndex::new(&volume, rs.as_bytes(), b'\\');
    assert_eq!(fresh.changes(&volume, Checkpoint { journal: 1, next: 2 }, &mut v).unwrap(), None, "a checkpoint from another process forces a rescan");
    let cp = fresh.scan(&volume, &mut v).unwrap();
    v.commit();
    let f = (0..v.len() as u32).find(|&i| v.name(i) == b"f.txt").unwrap();
    assert_eq!(v.sizes[f as usize], 3);
    assert!(matches!(fresh.modes.get(&volume), Some(Mode::Walk)));
    let next = fresh.changes(&volume, cp, &mut v).unwrap().expect("a walked volume follows its journal");
    assert!(next.journal == cp.journal && next.next >= cp.next);
    drop(fresh);
    std::fs::remove_dir_all(&dir).unwrap();
}
