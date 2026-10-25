use super::*;
use flit_core::listing::Entry;
use std::path::PathBuf;
use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY};

pub(crate) struct FakeBin {
    top: PathBuf,
    pub root: PathBuf,
}

impl FakeBin {
    pub fn new(tag: &str) -> Self {
        let top = std::env::temp_dir().join(format!("flit-bin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let root = top.join(r"$Recycle.Bin\S-1-5-21-0-0-0-1001");
        std::fs::create_dir_all(&root).unwrap();
        Self { top, root }
    }

    pub fn path(&self) -> FsPath {
        FsPath::parse(self.root.to_str().unwrap()).unwrap()
    }

    pub fn recycle(&self, id: &str, original: &str, version: u64, dir: bool) -> FsPath {
        let payload = self.root.join(format!("$R{id}"));
        if dir {
            std::fs::create_dir_all(payload.join("inner")).unwrap();
            std::fs::write(payload.join(r"inner\f.txt"), b"x").unwrap();
        } else {
            std::fs::write(&payload, b"payload").unwrap();
        }
        std::fs::write(self.root.join(format!("$I{id}")), info_bytes(version, 7, 1000, original)).unwrap();
        FsPath::parse(payload.to_str().unwrap()).unwrap()
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    }
}

impl Drop for FakeBin {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.top);
    }
}

pub(crate) fn info_bytes(version: u64, size: u64, deleted: i64, path: &str) -> Vec<u8> {
    let mut units: Vec<u16> = path.encode_utf16().collect();
    let mut b: Vec<u8> = [version.to_le_bytes(), size.to_le_bytes(), deleted.to_le_bytes()].concat();
    if version == 1 {
        units.resize(V1_PATH_UNITS, 0);
    } else {
        units.push(0);
        b.extend((units.len() as u32).to_le_bytes());
    }
    b.extend(units.iter().flat_map(|c| c.to_le_bytes()));
    b
}

fn set_attrs(p: &std::path::Path, attrs: u32) {
    let w: Vec<u16> = p.to_str().unwrap().encode_utf16().chain(Some(0)).collect();
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(attrs)) }.unwrap();
}

fn listed(roots: &[FsPath]) -> Listing {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    list(roots, &mut b).unwrap();
    b.listing
}

fn find<'a>(l: &'a Listing, name: &str) -> (u32, &'a Entry) {
    let i = (0..l.len() as u32).find(|&i| l.name(i) == name.as_bytes()).unwrap_or_else(|| panic!("{name} is not listed"));
    (i, &l.entries[i as usize])
}

#[test]
fn info_files_of_both_versions_are_parsed() {
    let long = format!(r"C:\{}\отчёт.txt", "d".repeat(400));
    for (version, path) in [(1, r"C:\Users\me\a.txt"), (2, r"C:\Users\me\a.txt"), (2, long.as_str())] {
        let info = parse_info(&info_bytes(version, 42, -5, path)).unwrap();
        assert_eq!((info.size, info.deleted, String::from_utf16(&info.path).unwrap()), (42, -5, path.to_owned()), "v{version}");
    }
    let v2 = info_bytes(2, 1, 1, r"C:\x");
    assert!(parse_info(&v2[..v2.len() - 4]).is_none(), "a v2 path shorter than its length is rejected");
    assert!(parse_info(&info_bytes(3, 1, 1, r"C:\x")).is_none());
    assert!(parse_info(&info_bytes(1, 1, 1, "")).is_none());
    assert!(parse_info(&v2[..20]).is_none());
}

#[test]
fn payloads_are_paired_with_their_info_files() {
    let p = |s: &str| FsPath::parse(s).unwrap();
    assert_eq!(info_file(&p(r"C:\$Recycle.Bin\S-1\$RAB12CD.txt")), Some(p(r"C:\$Recycle.Bin\S-1\$IAB12CD.txt")));
    assert_eq!(info_file(&p(r"C:\temp\S-1\$RAB12CD.txt")), None);
    assert_eq!(info_file(&p(r"C:\$Recycle.Bin\S-1\notes.txt")), None);
}

#[test]
fn every_fixed_drive_has_a_bin_root_for_the_current_user() {
    let sid = user_sid().unwrap();
    let roots = roots();
    let system = std::env::var("SystemDrive").unwrap();
    assert!(roots.iter().any(|r| r.as_bytes().starts_with(system.as_bytes())), "{roots:?}");
    for r in &roots {
        assert_eq!(r.file_name(), sid.as_bytes());
        assert!(r.parent().unwrap().file_name().eq_ignore_ascii_case(BIN_FOLDER));
    }
}

#[test]
fn a_fake_bin_lists_original_names_locations_dates_and_sizes() {
    let bin = FakeBin::new("list");
    let report = bin.recycle("AB12CD.txt", r"C:\Users\me\Документы\отчёт.txt", 2, false);
    bin.recycle("000001", r"D:\old", 1, true);
    bin.recycle("ZZ.log", r"E:\root.log", 2, false);
    std::fs::write(bin.root.join("$IORPHAN.txt"), info_bytes(2, 1, 1, r"C:\gone.txt")).unwrap();
    std::fs::write(bin.root.join("$RLONELY"), b"").unwrap();
    std::fs::write(bin.root.join("$IBROKEN"), b"junk").unwrap();
    std::fs::write(bin.root.join("$RBROKEN"), b"").unwrap();
    std::fs::write(bin.root.join("desktop.ini"), b"").unwrap();
    set_attrs(&bin.root.join("$RAB12CD.txt"), FILE_ATTRIBUTE_HIDDEN.0);
    let missing = FsPath::parse(r"C:\no-such-flit-dir\$Recycle.Bin\S-1").unwrap();
    let l = listed(&[missing, bin.path()]);
    assert_eq!(l.len(), 3);
    assert_eq!(count(&[bin.path()]), 3);
    let (i, e) = find(&l, "отчёт.txt");
    assert_eq!((e.size, e.modified, e.is_dir()), (7, 1000, false));
    assert_eq!(e.attrs & (attr::HIDDEN | attr::SIZE_KNOWN), attr::SIZE_KNOWN, "a hidden item still shows in the bin");
    assert_eq!(bin::recycled(&l, i), Some(report));
    assert_eq!(bin::origin(&l, i), r"C:\Users\me\Документы".as_bytes());
    assert_eq!(l.ext(i), b"txt");
    let (i, e) = find(&l, "old");
    assert!(e.is_dir());
    assert_eq!(bin::origin(&l, i), br"D:\");
    let (i, _) = find(&l, "root.log");
    assert_eq!(bin::original(&l, i), FsPath::parse(r"E:\root.log"));
}

#[test]
fn purging_removes_payload_and_info_and_refuses_paths_outside_a_bin() {
    let bin = FakeBin::new("purge");
    let file = bin.recycle("F1.txt", r"C:\a\f.txt", 2, false);
    let dir = bin.recycle("D1", r"C:\a\d", 2, true);
    bin.recycle("KEEP.txt", r"C:\a\keep.txt", 2, false);
    set_attrs(&bin.root.join("$RF1.txt"), FILE_ATTRIBUTE_READONLY.0);
    set_attrs(&bin.root.join(r"$RD1\inner\f.txt"), FILE_ATTRIBUTE_READONLY.0);
    let original = |s: &str| FsPath::parse(s).unwrap();
    let out = purge(&[(file.clone(), original(r"C:\a\f.txt")), (dir.clone(), original(r"C:\a\d"))]);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 2);
    assert_eq!(bin.names(), ["$IKEEP.txt", "$RKEEP.txt"]);

    let outside = bin.top.join("$Rplain.txt");
    std::fs::write(&outside, b"mine").unwrap();
    let outside = FsPath::parse(outside.to_str().unwrap()).unwrap();
    let out = purge(&[(outside.clone(), outside.clone())]);
    assert!(out.error.is_some() && out.items.is_empty());
    assert!(bin.top.join("$Rplain.txt").exists(), "nothing outside a bin is ever deleted");
}

#[test]
fn emptying_a_fake_bin_removes_items_and_orphans_but_keeps_desktop_ini() {
    let bin = FakeBin::new("empty");
    bin.recycle("A.txt", r"C:\a.txt", 2, false);
    bin.recycle("B", r"C:\b", 1, true);
    std::fs::write(bin.root.join("$IORPHAN.txt"), info_bytes(2, 1, 1, r"C:\gone.txt")).unwrap();
    std::fs::write(bin.root.join("$RLONELY"), b"").unwrap();
    std::fs::write(bin.root.join("desktop.ini"), b"").unwrap();
    let out = empty(&[bin.path()]);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(bin.names(), ["desktop.ini"]);
    assert_eq!(count(&[bin.path()]), 0);
}
