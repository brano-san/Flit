use super::*;
use flit_search::query::{parse, Clock};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static DISK: RefCell<(HashMap<String, FileKey>, u64)> = RefCell::new((HashMap::new(), 0));
}

fn volume_of(p: &str) -> Option<u32> {
    match p.as_bytes()[0].to_ascii_uppercase() {
        b'C' => Some(1),
        b'D' => Some(2),
        _ => None,
    }
}

fn create(p: &str) {
    DISK.with(|d| {
        let (files, next) = &mut *d.borrow_mut();
        *next += 1;
        files.insert(p.to_lowercase(), FileKey { volume: volume_of(p).unwrap_or(0), id: *next });
    });
}

fn rename(from: &str, to: &str) {
    DISK.with(|d| {
        let (files, next) = &mut *d.borrow_mut();
        let moved: Vec<(String, FileKey)> = files.iter().filter(|(p, _)| *p == &from.to_lowercase() || p.starts_with(&format!("{}\\", from.to_lowercase()))).map(|(p, k)| (p.clone(), *k)).collect();
        for (p, k) in moved {
            files.remove(&p);
            let dest = format!("{}{}", to.to_lowercase(), &p[from.len()..]);
            let key = if volume_of(&dest) == Some(k.volume) {
                k
            } else {
                *next += 1;
                FileKey { volume: volume_of(&dest).unwrap_or(0), id: *next }
            };
            files.insert(dest, key);
        }
    });
}

fn remove(p: &str) {
    DISK.with(|d| d.borrow_mut().0.remove(&p.to_lowercase()));
}

fn fake_key(p: &FsPath) -> Option<FileKey> {
    let s = p.display().to_lowercase();
    volume_of(&s)?;
    DISK.with(|d| d.borrow().0.get(&s).copied())
}

fn fake_resolve(keys: &[FileKey]) -> Vec<Resolved> {
    DISK.with(|d| {
        let files = &d.borrow().0;
        keys.iter()
            .map(|k| match files.iter().find(|(_, v)| *v == k) {
                Some((p, _)) => Resolved::Found(FsPath::parse(p).unwrap(), Meta { size: p.len() as u64, ..Meta::default() }),
                None if k.volume == 9 => Resolved::Unavailable,
                None => Resolved::Gone,
            })
            .collect()
    })
}

fn path(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

struct TempFile(PathBuf);

impl TempFile {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("flit-tagbook-{tag}-{}", std::process::id())).join("tags.bin");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        Self(p)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn book(file: Option<PathBuf>) -> Book {
    Book::load(file, fake_key, fake_resolve)
}

fn masks(b: &Book) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = b.store.files.values().map(|t| (t.path.display().to_lowercase(), t.mask)).collect();
    v.sort();
    v
}

#[test]
fn tags_are_assigned_cleared_deleted_and_survive_a_restart() {
    let f = TempFile::new("roundtrip");
    for p in [r"c:\t\a.txt", r"c:\t\b.txt"] {
        create(p);
    }
    let mut b = book(Some(f.0.clone()));
    assert_eq!(b.store.tags.len(), 1);
    let o = b.apply(TagRequest::Create { name: "Work".into(), color: 7, assign: vec![path(r"c:\t\a.txt"), path(r"c:\t\b.txt"), path(r"e:\x.txt")] });
    assert_eq!((o.changed, o.files.len(), o.status.as_deref()), (true, 2, Some(NOT_TAGGABLE)));
    let o = b.apply(TagRequest::Set { items: vec![path(r"c:\t\a.txt")], tag: 0, on: true });
    assert_eq!(o.files, [(path(r"c:\t\a.txt"), 0b11)]);
    assert_eq!(b.apply(TagRequest::Set { items: vec![path(r"c:\t\a.txt")], tag: 5, on: true }).status.as_deref(), Some("This tag no longer exists"));
    let o = b.apply(TagRequest::Clear(vec![path(r"c:\t\b.txt"), path(r"c:\t\untagged.txt")]));
    assert_eq!(o.files, [(path(r"c:\t\b.txt"), 0)]);
    assert_eq!(b.apply(TagRequest::Rename { tag: 1, name: "important".into() }).status.as_deref(), Some("A tag with this name already exists"));
    b.apply(TagRequest::Recolor { tag: 1, color: 9 });
    let again = book(Some(f.0.clone()));
    assert_eq!(again.store, b.store);
    assert_eq!(masks(&again), [(r"c:\t\a.txt".into(), 0b11)]);
    let o = b.apply(TagRequest::Delete(0));
    assert_eq!((o.dropped, masks(&b)), (Some(0), vec![(r"c:\t\a.txt".into(), 0b1)]));
    assert_eq!(b.store.tags[0].color, 9);
    let published = b.publish();
    assert!(published.version > 0 && published.files[0].present);
    std::fs::write(&f.0, b"not a tag file").unwrap();
    assert_eq!(book(Some(f.0.clone())).store, TagStore::default());
    assert!(f.0.with_extension("bin.bad").exists(), "an unreadable store is kept aside, not overwritten");
}

#[test]
fn moves_and_copies_keep_tags_and_rechecks_follow_the_disk() {
    for p in [r"c:\t\dir", r"c:\t\dir\in.txt", r"c:\t\one.txt", r"c:\t\two.txt", r"c:\t\gone.txt"] {
        create(p);
    }
    let mut b = book(None);
    let all = [r"c:\t\dir", r"c:\t\dir\in.txt", r"c:\t\one.txt", r"c:\t\two.txt", r"c:\t\gone.txt"].map(path).to_vec();
    b.apply(TagRequest::Set { items: all, tag: 0, on: true });
    rename(r"c:\t\dir", r"c:\t\moved");
    rename(r"c:\t\one.txt", r"d:\one.txt");
    let pairs = vec![(path(r"c:\t\dir"), path(r"c:\t\moved")), (path(r"c:\t\one.txt"), path(r"d:\one.txt"))];
    let o = b.apply(TagRequest::Moved { pairs, copy: false });
    assert!(o.files.contains(&(path(r"c:\t\moved\in.txt"), 1)) && o.files.contains(&(path(r"d:\one.txt"), 1)));
    create(r"c:\t\copy.txt");
    b.apply(TagRequest::Moved { pairs: vec![(path(r"c:\t\two.txt"), path(r"c:\t\copy.txt"))], copy: true });
    rename(r"c:\t\copy.txt", r"e:\copy.txt");
    b.apply(TagRequest::Moved { pairs: vec![(path(r"c:\t\copy.txt"), path(r"e:\copy.txt"))], copy: false });
    let quiet = b.apply(TagRequest::Moved { pairs: vec![(path(r"c:\elsewhere"), path(r"c:\other"))], copy: false });
    assert!(!quiet.changed, "moves of untagged items change nothing");
    let want = [r"c:\t\gone.txt", r"c:\t\moved", r"c:\t\moved\in.txt", r"c:\t\two.txt", r"d:\one.txt"];
    assert_eq!(masks(&b), want.map(|p| (p.to_owned(), 1)));
    remove(r"c:\t\gone.txt");
    rename(r"c:\t\two.txt", r"c:\t\renamed.txt");
    b.store.files.insert(FileKey { volume: 9, id: 1 }, Tagged { mask: 1, path: path(r"c:\offline\x") });
    assert!(b.recheck());
    assert!(!b.recheck(), "nothing changed since");
    let want = [r"c:\offline\x", r"c:\t\moved", r"c:\t\moved\in.txt", r"c:\t\renamed.txt", r"d:\one.txt"];
    assert_eq!(masks(&b), want.map(|p| (p.to_owned(), 1)));
    let t = b.publish();
    assert_eq!(t.files.iter().filter(|f| !f.present).map(|f| f.path.display().to_lowercase()).collect::<Vec<_>>(), [r"c:\offline\x"]);
}

#[test]
fn the_command_line_creates_missing_tags() {
    create(r"c:\t\cli.txt");
    let mut b = book(None);
    let o = b.apply(TagRequest::Command { name: "Travel".into(), items: vec![path(r"c:\t\cli.txt")] });
    assert_eq!(o.status.as_deref(), Some("Tagged 1 item with Travel"));
    assert_eq!((b.store.tags[1].name.as_str(), b.store.tags[1].color), ("Travel", COLORS[1].1));
    let o = b.apply(TagRequest::Command { name: "important".into(), items: vec![path(r"c:\t\cli.txt")] });
    assert_eq!((o.status.as_deref(), b.store.tags.len()), (Some("Tagged 1 item with Important"), 2));
    assert_eq!(masks(&b), [(r"c:\t\cli.txt".into(), 0b11)]);
    assert!(b.apply(TagRequest::Command { name: "".into(), items: Vec::new() }).status.is_some());
}

fn sample() -> Tags {
    let mut s = TagStore::default();
    s.add("Work", 0).unwrap();
    s.add("Old", 0).unwrap();
    let files = [(r"C:\docs\report.pdf", 0b011, 300, false), (r"C:\docs\notes.txt", 0b010, 100, false), (r"C:\docs\old", 0b110, 0, true), (r"D:\Report final.PDF", 0b001, 200, false), (r"C:\gone.pdf", 0b010, 1, false)];
    let mut seen = FxHashMap::default();
    for (k, (p, mask, size, dir)) in files.into_iter().enumerate() {
        let key = FileKey { volume: 1, id: k as u64 };
        s.update(key, path(p), |_| mask);
        let attrs = if dir { flit_core::attr::DIR } else { 0 };
        seen.insert(key, (Meta { size, modified: k as i64, attrs, ..Meta::default() }, k != 4));
    }
    Tags::new(1, &s, &seen)
}

fn found(t: &Tags, text: &str, key: SearchKey, descending: bool) -> Result<Vec<String>, String> {
    let q = parse(text, Clock::system(0)).unwrap();
    Ok(search(t, &q, SearchSort { key, descending })?.into_iter().map(|r| format!("{}\\{}", String::from_utf8_lossy(&r.dir).trim_end_matches('\\'), String::from_utf8_lossy(&r.name))).collect())
}

#[test]
fn tag_queries_combine_with_the_rest_of_the_syntax() {
    let t = sample();
    let q = |text: &str| found(&t, text, SearchKey::Name, false).unwrap();
    assert_eq!(q("tag:work"), [r"C:\docs\notes.txt", r"C:\docs\old", r"C:\docs\report.pdf"]);
    assert_eq!(q("tag:Work AND NOT tag:Old"), [r"C:\docs\notes.txt", r"C:\docs\report.pdf"]);
    assert_eq!(q("tag:Work !tag:old ext:pdf"), [r"C:\docs\report.pdf"]);
    assert_eq!(q("tag: report"), [r"D:\Report final.PDF", r"C:\docs\report.pdf"]);
    assert_eq!(q("tag:Important path:d:\\"), [r"D:\Report final.PDF"]);
    assert_eq!(q("tag: size:>150"), [r"D:\Report final.PDF", r"C:\docs\report.pdf"]);
    assert_eq!(q("tag: *.pdf"), [r"D:\Report final.PDF", r"C:\docs\report.pdf"]);
    assert_eq!(q("NOT tag:Important"), [r"C:\docs\notes.txt", r"C:\docs\old"]);
    assert_eq!(found(&t, "tag:", SearchKey::Size, true).unwrap()[0], r"C:\docs\report.pdf");
    assert_eq!(found(&t, "tag:", SearchKey::Folder, false).unwrap(), [r"C:\docs\notes.txt", r"C:\docs\old", r"C:\docs\report.pdf", r"D:\Report final.PDF"]);
    assert_eq!(found(&t, "tag:", SearchKey::Modified, true).unwrap()[0], r"D:\Report final.PDF");
    assert_eq!(found(&t, "tag:Nope", SearchKey::Name, false), Err("No tag named \"Nope\"".into()));
}

#[test]
#[ignore]
fn bench_tag_queries_over_10k_files() {
    let mut s = TagStore::default();
    s.add("Work", 0).unwrap();
    s.add("Old", 0).unwrap();
    let mut seen = FxHashMap::default();
    for i in 0..10_000u64 {
        let key = FileKey { volume: 1, id: i };
        let ext = ["pdf", "txt", "docx", "jpg"][(i % 4) as usize];
        s.update(key, path(&format!(r"C:\data\project {}\report {i}.{ext}", i % 100)), |_| (1 + i % 7) as u32);
        seen.insert(key, (Meta { size: i * 1000, modified: i as i64, ..Meta::default() }, true));
    }
    let t = Tags::new(1, &s, &seen);
    for text in ["tag:Work", "tag:Work AND NOT tag:Old", "tag: *.pdf", "tag:Important report 99 size:>1mb"] {
        let q = parse(text, Clock::system(0)).unwrap();
        let mut times: Vec<std::time::Duration> = (0..15)
            .map(|_| {
                let at = std::time::Instant::now();
                assert!(!search(&t, &q, SearchSort::default()).unwrap().is_empty());
                at.elapsed()
            })
            .collect();
        times.sort();
        eprintln!("BENCH {:<44} median {:>9.3} ms   max {:>9.3} ms", format!("{text} over 10k tagged files"), times[7].as_secs_f64() * 1e3, times[14].as_secs_f64() * 1e3);
    }
}
