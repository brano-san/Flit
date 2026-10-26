use super::*;
use std::time::Duration;

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
    let got = notices(&buf).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(String::from_utf16(&got[0].name).unwrap(), "long name.txt");
    assert_eq!(got[0].kind, ChangeKind::Added);
    assert_eq!(got[1].name, [b'a' as u16]);
    assert_eq!(got[1].kind, ChangeKind::Modified);
    assert_eq!((got[1].meta.size, got[1].meta.modified, got[1].meta.created), (42, 9, 7));
}

#[test]
fn unknown_records_are_skipped_and_truncated_buffers_ask_for_a_rescan() {
    let mut buf = record(FILE_ACTION(99), "skip", 0, 104);
    buf.extend(record(FILE_ACTION_REMOVED, "gone", 0, 0));
    let got = notices(&buf).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, ChangeKind::Removed);
    let cut = record(FILE_ACTION_ADDED, "abcdef", 0, 0);
    assert!(notices(&cut[..cut.len() - 1]).is_none());
    let whole = record(FILE_ACTION_ADDED, "kept", 0, 0).len() as u32;
    let mut partial = record(FILE_ACTION_ADDED, "kept", 0, whole);
    partial.extend(&cut[..cut.len() - 1]);
    assert!(notices(&partial).is_none(), "a good record before a cut one does not make a partial list");
    let mut beyond = record(FILE_ACTION_ADDED, "kept", 0, whole);
    beyond.extend(record(FILE_ACTION_ADDED, "lost", 0, 0));
    let at = offset_of!(Record, NextEntryOffset);
    beyond[at..at + 4].copy_from_slice(&4096u32.to_le_bytes());
    assert!(notices(&beyond).is_none(), "a next offset past the buffer is an overflow");
    assert!(notices(&[]).is_none());
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

fn temp_dir(tag: &str) -> (std::path::PathBuf, FsPath) {
    let root = std::env::temp_dir().join(format!("flit-watch-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = FsPath::parse(root.to_str().unwrap()).unwrap();
    (root, path)
}

#[test]
fn a_stopped_watch_lives_until_its_queued_completion_is_dequeued() {
    let (root, dir) = temp_dir("stop");
    let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) }.unwrap();
    let h = open_dir(&dir, FILE_LIST_DIRECTORY.0, true).unwrap();
    let key = 7;
    unsafe { CreateIoCompletionPort(h, Some(port), key, 0) }.unwrap();
    let mut st = State::default();
    st.slots.insert(key, Watch { tab: 1, gen: 1, path: dir, dir: h, ov: Box::default(), buf: vec![0u64; BUFFER / 8], closing: false, rescan: false });
    assert!(st.slots.get_mut(&key).unwrap().issue());
    std::fs::write(root.join("x"), b"1").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    st.stop(key);
    assert!(st.slots.contains_key(&key), "the buffer and OVERLAPPED stay alive while the completion is still queued");
    let (mut n, mut got, mut ov) = (0u32, 0usize, std::ptr::null_mut::<OVERLAPPED>());
    let ok = unsafe { GetQueuedCompletionStatus(port, &mut n, &mut got, &mut ov, 5000) }.is_ok();
    assert_eq!(got, key);
    assert_eq!(ov as *const OVERLAPPED, &*st.slots[&key].ov as *const OVERLAPPED);
    if ok && n > 0 {
        assert!(notices(st.slots[&key].bytes(n as usize)).unwrap().iter().any(|x| x.name == [b'x' as u16]));
    }
    st.close(key);
    assert!(st.slots.is_empty());
    unsafe {
        let _ = CloseHandle(port);
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn next_event(rx: &crossbeam_channel::Receiver<ProviderEvent>) -> ProviderEvent {
    rx.recv_timeout(Duration::from_secs(5)).expect("no watcher event")
}

#[test]
fn bin_watches_cover_every_root_and_only_ask_for_a_rescan() {
    let ((a, pa), (b, pb)) = (temp_dir("bin-a"), temp_dir("bin-b"));
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut w = DirWatcher::new(tx, Arc::new(|| {})).unwrap();
    w.watch_all(3, 9, Some(&[pa, pb, FsPath::parse(r"C:\no-such-flit-dir").unwrap()]), true);
    assert_eq!(lock(&w.state).by_tab[&3].len(), 2);
    std::fs::write(b.join("$Rx"), b"1").unwrap();
    assert!(matches!(next_event(&rx), ProviderEvent::Rescan { tab: 3, gen: 9 }));
    while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
    std::fs::write(a.join("$Iy"), b"1").unwrap();
    assert!(matches!(next_event(&rx), ProviderEvent::Rescan { tab: 3, gen: 9 }));
    w.watch(3, 9, None);
    assert!(!lock(&w.state).by_tab.contains_key(&3));
    drop(w);
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
