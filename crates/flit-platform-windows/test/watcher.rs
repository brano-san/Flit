use super::*;

fn record(action: FILE_ACTION, name: &str, size: i64, next: u32) -> Vec<u8> {
    let wide: Vec<u16> = name.encode_utf16().collect();
    let mut r = vec![0u8; offset_of!(Record, FileName)];
    let mut put = |at: usize, bytes: &[u8]| r[at..at + bytes.len()].copy_from_slice(bytes);
    put(offset_of!(Record, NextEntryOffset), &next.to_le_bytes());
    put(offset_of!(Record, Action), &action.0.to_le_bytes());
    put(offset_of!(Record, CreationTime), &7i64.to_le_bytes());
    put(offset_of!(Record, LastModificationTime), &9i64.to_le_bytes());
    put(offset_of!(Record, FileSize), &size.to_le_bytes());
    put(offset_of!(Record, FileAttributes), &FILE_ATTRIBUTE_ARCHIVE.0.to_le_bytes());
    put(offset_of!(Record, FileNameLength), &(wide.len() as u32 * 2).to_le_bytes());
    r.extend(wide.iter().flat_map(|c| c.to_le_bytes()));
    if next != 0 {
        r.resize(next as usize, 0);
    }
    r
}

#[test]
fn one_char_name_in_final_record_is_parsed() {
    let mut buf = record(FILE_ACTION_ADDED, "long name.txt", 5, 120);
    let last = record(FILE_ACTION_MODIFIED, "a", 42, 0);
    assert_eq!(last.len(), 86);
    assert!(last.len() < size_of::<Record>());
    buf.extend(last);
    let got = notices(&buf);
    assert_eq!(got.len(), 2);
    assert_eq!(String::from_utf16(&got[0].name).unwrap(), "long name.txt");
    assert_eq!(got[0].kind, ChangeKind::Added);
    assert_eq!(got[1].name, [b'a' as u16]);
    assert_eq!(got[1].kind, ChangeKind::Modified);
    assert_eq!((got[1].meta.size, got[1].meta.modified, got[1].meta.created), (42, 9, 7));
}

#[test]
fn truncated_and_unknown_records_are_dropped() {
    let mut buf = record(FILE_ACTION(99), "skip", 0, 104);
    buf.extend(record(FILE_ACTION_REMOVED, "gone", 0, 0));
    let got = notices(&buf);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, ChangeKind::Removed);
    let cut = record(FILE_ACTION_ADDED, "abcdef", 0, 0);
    assert!(notices(&cut[..cut.len() - 1]).is_empty());
    assert!(notices(&[]).is_empty());
}

#[test]
fn short_alias_shape() {
    let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    for alias in ["LONGFI~1.TXT", "PROGRA~1", "AB12~10.HTM", "A~1"] {
        assert!(short_alias(&w(alias)), "{alias}");
    }
    for name in ["~$report.docx", "backup~", "notes.txt~", "a~b.txt", "VERYLONGNAME~1.TXT", "LONGFI~1.TEXT", "a.b~1.txt", "~"] {
        assert!(!short_alias(&w(name)), "{name}");
    }
}

fn wait_for(rx: &crossbeam_channel::Receiver<ProviderEvent>, want: impl Fn(&DirChange) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Ok(ev) = rx.recv_deadline(deadline) {
        if let ProviderEvent::Changes { changes, .. } = ev {
            if changes.iter().any(&want) {
                return true;
            }
        }
    }
    false
}

#[test]
fn armed_before_watch_returns_and_resolves_short_names() {
    let root = std::env::temp_dir().join(format!("flit-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let long = root.join("long file name.txt");
    std::fs::write(&long, b"x").unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut w = DirWatcher::new(tx, Arc::new(|| {})).unwrap();
    w.watch(1, 1, Some(&FsPath::parse(root.to_str().unwrap()).unwrap()));
    std::fs::write(root.join("a"), b"1").unwrap();
    assert!(wait_for(&rx, |c| c.name == b"a" && c.kind == ChangeKind::Added));
    let wide: Vec<u16> = long.to_str().unwrap().encode_utf16().chain(Some(0)).collect();
    let mut short = vec![0u16; 1024];
    let n = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut short)) } as usize;
    let short = String::from_utf16(&short[..n]).unwrap();
    if n > 0 && short.contains('~') {
        std::fs::write(&short, b"longer").unwrap();
        assert!(wait_for(&rx, |c| c.name == b"long file name.txt" && c.kind == ChangeKind::Modified), "{short}");
    }
    w.watch(1, 1, None);
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}
