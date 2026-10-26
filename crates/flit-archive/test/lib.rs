use super::*;
use flit_core::archive::{Compression, Encoding, Format, Place, Secret};
use flit_core::provider::Resolve;
use flit_core::sort::CaseMode;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("flit-archive-test-{}-{}-{tag}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn fs(&self, rel: &str) -> FsPath {
        let p = if rel.is_empty() { self.0.clone() } else { self.0.join(rel) };
        FsPath::parse(p.to_str().unwrap()).unwrap()
    }
    fn write(&self, rel: &str, data: &[u8]) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }
    fn read(&self, rel: &str) -> Vec<u8> {
        fs::read(self.path(rel)).unwrap_or_default()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unlimited(_: &Path) -> Option<u64> {
    Some(u64::MAX)
}

fn env() -> Env<'static> {
    Env { utc_offset: 0, free_space: &unlimited }
}

fn run(op: FileOp, policy: Resolve) -> OpOutcome {
    file_op(&op, &Job::detached(policy), &env()).expect("an archive operation")
}

fn names(dir: &FsPath) -> Result<Vec<(String, u64, bool)>, FsError> {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    read_dir(dir, &mut b, 0).expect("an archive path")?;
    let l = &b.listing;
    let mut v: Vec<_> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), l.entries[i as usize].size, l.entries[i as usize].is_dir())).collect();
    v.sort();
    Ok(v)
}

fn sample(t: &Temp) -> Vec<FsPath> {
    t.write("src/Документы/отчёт.txt", "квартальный отчёт".as_bytes());
    t.write("src/Документы/deep/data.bin", &(0..50_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect::<Vec<_>>());
    t.write("src/readme.md", b"hello archive");
    fs::create_dir_all(t.path("src/empty")).unwrap();
    vec![t.fs("src/Документы"), t.fs("src/readme.md"), t.fs("src/empty")]
}

fn compress(items: Vec<FsPath>, archive: FsPath, format: Format, password: Option<&str>) {
    let options = Compression { format, level: format.default_level(), password: password.map(|p| Secret(p.into())), encrypt_names: password.is_some(), ..Compression::default() };
    let out = run(FileOp::Archive(ArchiveOp::Compress { items, archive: archive.clone(), options }), Resolve::Replace);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 1);
}

fn extract(archive: &FsPath, dest: &FsPath, place: Place) -> OpOutcome {
    run(FileOp::Archive(ArchiveOp::Extract { archives: vec![archive.clone()], dest: Some(dest.clone()), place }), Resolve::Replace)
}

fn roundtrip(format: Format, ext: &str) {
    let t = Temp::new(ext);
    let items = sample(&t);
    let archive = t.fs(&format!("a.{ext}"));
    compress(items, archive.clone(), format, None);
    let root = names(&archive).unwrap();
    assert_eq!(root.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>(), ["empty", "readme.md", "Документы"]);
    assert!(root[2].2 && root[2].1 == 200_000 + "квартальный отчёт".len() as u64);
    let deep = names(&archive.join("Документы".as_bytes()).join(b"deep")).unwrap();
    assert_eq!(deep, vec![("data.bin".to_owned(), 200_000, false)]);
    let out = extract(&archive, &t.fs("out"), Place::OwnFolder);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, vec![(archive.clone(), t.fs("out/a"))]);
    assert_eq!(t.read("out/a/Документы/отчёт.txt"), "квартальный отчёт".as_bytes());
    assert_eq!(t.read("out/a/Документы/deep/data.bin"), t.read("src/Документы/deep/data.bin"));
    assert!(t.path("out/a/empty").is_dir());
}

#[test]
fn zip_7z_and_tar_round_trip_through_folders() {
    roundtrip(Format::Zip, "zip");
    roundtrip(Format::SevenZ, "7z");
    roundtrip(Format::Tar, "tar");
    roundtrip(Format::TarGz, "tar.gz");
}

#[test]
fn gzip_holds_one_file_and_several_become_tar_gz() {
    let t = Temp::new("gz");
    t.write("big.log", &b"line of text\n".repeat(10_000));
    compress(vec![t.fs("big.log")], t.fs("big.log.gz"), Format::Gz, None);
    assert_eq!(names(&t.fs("big.log.gz")).unwrap(), vec![("big.log".to_owned(), 130_000, false)]);
    let out = extract(&t.fs("big.log.gz"), &t.fs("out"), Place::Here);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(t.read("out/big.log"), t.read("big.log"));
    let items = sample(&t);
    compress(items, t.fs("many.gz"), Format::Gz, None);
    assert_eq!(names(&t.fs("many.gz")).unwrap().len(), 3);
}

#[test]
fn passwords_protect_zip_and_7z() {
    for (format, ext) in [(Format::Zip, "zip"), (Format::SevenZ, "7z")] {
        let t = Temp::new(ext);
        let items = sample(&t);
        let archive = t.fs(&format!("secret.{ext}"));
        compress(items, archive.clone(), format, Some("correct horse"));
        if format == Format::SevenZ {
            assert_eq!(names(&archive).unwrap_err().code, PASSWORD_CODE);
        }
        let out = extract(&archive, &t.fs("none"), Place::Here);
        assert_eq!(out.error.map(|e| e.code), Some(PASSWORD_CODE));
        archive::set_password(&archive, Some(Secret("wrong".into())));
        let out = extract(&archive, &t.fs("wrong"), Place::Here);
        assert_eq!(out.error.map(|e| e.message), Some(flit_core::i18n::S::ErrorsArchiveWrongPassword.en().to_owned()), "{ext}");
        archive::set_password(&archive, Some(Secret("correct horse".into())));
        assert_eq!(names(&archive).unwrap().len(), 3);
        let out = extract(&archive, &t.fs("ok"), Place::Here);
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(t.read("ok/readme.md"), b"hello archive");
    }
}

pub fn raw_zip(entries: &[(&[u8], &[u8], u16)]) -> Vec<u8> {
    let (mut out, mut cd) = (Vec::new(), Vec::new());
    for &(name, data, flags) in entries {
        let crc = crc32fast::hash(data);
        let at = out.len() as u32;
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes());
            v.extend_from_slice(&flags.to_le_bytes());
            v.extend_from_slice(&[0, 0, 0, 0, 0x21, 0x58]);
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        common(&mut cd);
        cd.extend_from_slice(&[0; 6]);
        cd.extend_from_slice(&0x20u32.to_le_bytes());
        cd.extend_from_slice(&at.to_le_bytes());
        cd.extend_from_slice(name);
    }
    let cd_at = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
fn hostile_names_never_leave_the_target() {
    let t = Temp::new("slip");
    let bad: [&[u8]; 9] = [b"../evil.txt", b"..\\evil2.txt", b"/abs.txt", b"C:/Windows/evil.dll", b"\\\\server\\share\\x", b"ok/file.txt:stream", b"ok/CON", b"ok/trailing. ", b"ok/link"];
    let mut entries: Vec<(&[u8], &[u8], u16)> = bad.iter().map(|n| (*n, &b"pwned"[..], 0)).collect();
    entries.push((b"ok/fine.txt", b"fine", 0));
    t.write("evil.zip", &raw_zip(&entries));
    let listed = names(&t.fs("evil.zip/ok")).unwrap();
    let unix_only: &[&str] = if cfg!(windows) { &[] } else { &["CON", "file.txt:stream", "trailing. "] };
    let mut want = [&["fine.txt", "link"][..], unix_only].concat();
    want.sort();
    let mut got: Vec<&str> = listed.iter().map(|(n, ..)| n.as_str()).collect();
    got.sort();
    assert_eq!(got, want);
    let out = extract(&t.fs("evil.zip"), &t.fs("x/target"), Place::Here);
    assert!(out.error.as_ref().is_some_and(|e| e.message.contains("unsafe names")), "{:?}", out.error);
    assert_eq!(t.read("x/target/ok/fine.txt"), b"fine");
    assert!(!t.path("x/evil.txt").exists() && !t.path("evil.txt").exists() && !t.path("x/evil2.txt").exists());
    let mut all = Vec::new();
    let mut stack = vec![t.path("x")];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                all.push(e.path());
            }
        }
    }
    assert_eq!(all.len(), if cfg!(windows) { 2 } else { 6 }, "Linux also extracts C:/Windows/evil.dll as a folder named C: {all:?}");
}

#[test]
fn decompression_bombs_and_full_disks_are_refused() {
    let t = Temp::new("bomb");
    let mut zip = raw_zip(&[(b"huge.bin", b"tiny", 0)]);
    let cd = zip.windows(4).position(|w| w == 0x0201_4b50u32.to_le_bytes()).unwrap();
    zip[cd + 24..cd + 28].copy_from_slice(&0xF000_0000u32.to_le_bytes());
    t.write("bomb.zip", &zip);
    let out = extract(&t.fs("bomb.zip"), &t.fs("out"), Place::Here);
    assert!(out.error.as_ref().is_some_and(|e| e.message.contains("decompression bomb")), "{:?}", out.error);
    assert!(!t.path("out/huge.bin").exists());
    let small = |_: &Path| Some(1000u64);
    t.write("data.txt", &vec![b'a'; 5000]);
    compress(vec![t.fs("data.txt")], t.fs("data.zip"), Format::Zip, None);
    let tight = Env { utc_offset: 0, free_space: &small };
    let op = FileOp::Archive(ArchiveOp::Extract { archives: vec![t.fs("data.zip")], dest: Some(t.fs("full")), place: Place::Here });
    let out = file_op(&op, &Job::detached(Resolve::Replace), &tight).unwrap();
    assert!(out.error.as_ref().is_some_and(|e| e.message.contains("Not enough free space")), "{:?}", out.error);
    compress(vec![t.fs("data.txt")], t.fs("data.txt.gz"), Format::Gz, None);
    let op = FileOp::Archive(ArchiveOp::Extract { archives: vec![t.fs("data.txt.gz")], dest: Some(t.fs("gz")), place: Place::Here });
    let out = file_op(&op, &Job::detached(Resolve::Replace), &tight).unwrap();
    assert!(out.error.as_ref().is_some_and(|e| e.message.contains("Not enough free space")), "{:?}", out.error);
    assert!(!t.path("gz/data.txt").exists());
}

#[test]
fn legacy_names_are_decoded_and_can_be_switched() {
    let t = Temp::new("cp866");
    let dos = |s: &str| encoding_rs::IBM866.encode(s).0.into_owned();
    let (a, b) = (dos("Отчёт за квартал.txt"), dos("Фото/лето.jpg"));
    t.write("old.zip", &raw_zip(&[(&a, b"1", 0), (&b, b"22", 0), ("Новый.txt".as_bytes(), b"3", 0x800)]));
    let zip = t.fs("old.zip");
    assert_eq!(names(&zip).unwrap().iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>(), ["Новый.txt", "Отчёт за квартал.txt", "Фото"]);
    assert_eq!(archive::detected_encoding(&zip), Some(Encoding::Cp866));
    archive::choose_encoding(&zip, Some(Encoding::Cp437));
    assert!(names(&zip).unwrap().iter().any(|(n, ..)| n.starts_with("ÄΓτ")));
    archive::choose_encoding(&zip, None);
    let out = extract(&zip, &t.fs("out"), Place::Here);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(t.read("out/Фото/лето.jpg"), b"22");
}

#[test]
fn files_are_added_to_zips_and_copied_out_of_them() {
    let t = Temp::new("add");
    let items = sample(&t);
    let zip = t.fs("a.zip");
    compress(items, zip.clone(), Format::Zip, None);
    t.write("new/note.txt", b"added");
    let dest = zip.join("Документы".as_bytes());
    let out = run(FileOp::Copy { items: vec![t.fs("new/note.txt")], dest: dest.clone() }, Resolve::Replace);
    assert!(out.error.is_none() && out.items.is_empty(), "{:?}", out.error);
    assert!(names(&dest).unwrap().iter().any(|(n, s, _)| n == "note.txt" && *s == 5));
    t.write("new/note.txt", b"replaced!");
    run(FileOp::Copy { items: vec![t.fs("new/note.txt")], dest: dest.clone() }, Resolve::Replace);
    run(FileOp::Copy { items: vec![t.fs("new/note.txt")], dest: dest.clone() }, Resolve::KeepBoth);
    let listed = names(&dest).unwrap();
    assert!(listed.contains(&("note.txt".to_owned(), 9, false)) && listed.contains(&("note (2).txt".to_owned(), 9, false)), "{listed:?}");
    let picks = vec![dest.join(b"note.txt"), dest.join(b"deep")];
    let out = run(FileOp::Copy { items: picks.clone(), dest: t.fs("copied") }, Resolve::Replace);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 2);
    assert!(out.items.contains(&(picks[0].clone(), t.fs("copied/note.txt"))) && out.items.contains(&(picks[1].clone(), t.fs("copied/deep"))), "{:?}", out.items);
    assert_eq!(t.read("copied/note.txt"), b"replaced!");
    assert_eq!(t.read("copied/deep/data.bin").len(), 200_000);
    let again = run(FileOp::Copy { items: vec![picks[0].clone()], dest: t.fs("copied") }, Resolve::KeepBoth);
    assert_eq!(again.items, vec![(picks[0].clone(), t.fs("copied/note (2).txt"))]);
    let refused = run(FileOp::Trash(vec![picks[0].clone()]), Resolve::Replace);
    assert_eq!(refused.error.map(|e| e.message), Some(flit_core::i18n::S::ErrorsArchiveInside.en().to_owned()));
    assert!(file_op(&FileOp::Trash(vec![zip.clone()]), &Job::detached(Resolve::Replace), &env()).is_none());
    t.write("b.7z", b"");
    compress(vec![t.fs("new/note.txt")], t.fs("b.7z"), Format::SevenZ, None);
    let out = run(FileOp::Copy { items: vec![t.fs("new/note.txt")], dest: t.fs("b.7z") }, Resolve::KeepBoth);
    assert_eq!(out.error.map(|e| e.message), Some(flit_core::i18n::S::ErrorsArchiveNoAdd.en().to_owned()));
}

#[test]
fn smart_extraction_avoids_double_folders() {
    let t = Temp::new("smart");
    t.write("one/inner/a.txt", b"a");
    compress(vec![t.fs("one")], t.fs("one.zip"), Format::Zip, None);
    t.write("two/x.txt", b"x");
    t.write("two/y.txt", b"y");
    compress(vec![t.fs("two/x.txt"), t.fs("two/y.txt")], t.fs("two.zip"), Format::Zip, None);
    let smart = |a: &str| run(FileOp::Archive(ArchiveOp::Extract { archives: vec![t.fs(a)], dest: Some(t.fs("out")), place: Place::Smart }), Resolve::Replace);
    assert_eq!(smart("one.zip").items, vec![(t.fs("one.zip"), t.fs("out/one"))]);
    assert_eq!(smart("two.zip").items, vec![(t.fs("two.zip"), t.fs("out/two"))]);
    assert_eq!(t.read("out/one/inner/a.txt"), b"a");
    assert_eq!(t.read("out/two/y.txt"), b"y");
}

#[test]
fn volumes_split_the_archive_into_parts() {
    let t = Temp::new("vol");
    let noise: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    t.write("noise.bin", &noise);
    let options = Compression { format: Format::Zip, level: 0, volume: 100_000, ..Compression::default() };
    let out = run(FileOp::Archive(ArchiveOp::Compress { items: vec![t.fs("noise.bin")], archive: t.fs("n.zip"), options }), Resolve::Replace);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), vec![t.fs("n.zip.001"), t.fs("n.zip.002"), t.fs("n.zip.003"), t.fs("n.zip.004")]);
    let joined: Vec<u8> = (1..=4).flat_map(|k| t.read(&format!("n.zip.{k:03}"))).collect();
    t.write("joined.zip", &joined);
    let mut z = zip::ZipArchive::new(fs::File::open(t.path("joined.zip")).unwrap()).unwrap();
    let mut back = Vec::new();
    z.by_index(0).unwrap().read_to_end(&mut back).unwrap();
    assert_eq!(back, noise);
}

pub fn raw_rar(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let block = |body: &[u8]| {
        let crc = (crc32fast::hash(body) & 0xFFFF) as u16;
        [&crc.to_le_bytes()[..], body].concat()
    };
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    out.extend(block(&[0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0]));
    for &(name, data, dir) in entries {
        let mut h = vec![0x74];
        h.extend_from_slice(&(0x8000u16 | if dir { 0xE0 } else { 0 }).to_le_bytes());
        h.extend_from_slice(&((32 + name.len()) as u16).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.push(2);
        h.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
        h.extend_from_slice(&0x5821_0000u32.to_le_bytes());
        h.extend_from_slice(&[20, 0x30]);
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&(if dir { 0x10u32 } else { 0x20 }).to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        out.extend(block(&h));
        out.extend_from_slice(data);
    }
    out.extend(block(&[0x7B, 0x00, 0x40, 7, 0]));
    out
}

#[test]
fn rar_archives_are_listed_and_extracted() {
    let t = Temp::new("rar");
    t.write("old.rar", &raw_rar(&[("docs", b"", true), ("docs\\a.txt", b"alpha", false), ("b.txt", b"beta!", false)]));
    let rar = t.fs("old.rar");
    assert_eq!(names(&rar).unwrap(), vec![("b.txt".to_owned(), 5, false), ("docs".to_owned(), 5, true)]);
    assert_eq!(names(&rar.join(b"docs")).unwrap(), vec![("a.txt".to_owned(), 5, false)]);
    let out = extract(&rar, &t.fs("out"), Place::Here);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(t.read("out/docs/a.txt"), b"alpha");
    assert_eq!(t.read("out/b.txt"), b"beta!");
}

#[test]
fn opening_an_entry_extracts_it_alone_to_a_fresh_temporary_folder() {
    let t = Temp::new("open");
    let items = sample(&t);
    let archive = t.fs("a.zip");
    compress(items, archive.clone(), Format::Zip, None);
    let item = archive.join("Документы".as_bytes()).join("отчёт.txt".as_bytes());
    let first = run(FileOp::Archive(ArchiveOp::Open { item: item.clone() }), Resolve::Skip);
    let second = run(FileOp::Archive(ArchiveOp::Open { item: item.clone() }), Resolve::Skip);
    assert!(first.error.is_none() && second.error.is_none(), "{:?}", first.error);
    let (from, to) = first.items[0].clone();
    assert_eq!((from, first.items.len()), (item, 1));
    assert_ne!(to, second.items[0].1);
    let opened = std_path(&to);
    assert_eq!(fs::read(&opened).unwrap(), "квартальный отчёт".as_bytes());
    assert_eq!(fs::read_dir(opened.parent().unwrap()).unwrap().count(), 1);
    for (_, to) in first.items.iter().chain(&second.items) {
        let _ = fs::remove_dir_all(std_path(to).parent().unwrap());
    }
    let outside = run(FileOp::Archive(ArchiveOp::Open { item: archive }), Resolve::Skip);
    assert!(outside.error.is_some());
}

#[cfg(windows)]
#[test]
fn unreadable_files_are_skipped_on_request() {
    use std::os::windows::fs::OpenOptionsExt;
    let t = Temp::new("locked");
    t.write("dir/free.txt", b"free");
    t.write("dir/locked.txt", b"locked");
    let _hold = fs::OpenOptions::new().read(true).share_mode(0).open(t.path("dir/locked.txt")).unwrap();
    for (format, ext) in [(Format::Zip, "zip"), (Format::SevenZ, "7z"), (Format::Tar, "tar")] {
        let archive = t.fs(&format!("d.{ext}"));
        let options = Compression { format, level: 1, ..Compression::default() };
        let out = run(FileOp::Archive(ArchiveOp::Compress { items: vec![t.fs("dir")], archive: archive.clone(), options }), Resolve::Skip);
        assert!(out.error.is_none(), "{ext}: {:?}", out.error);
        let listed = names(&archive.join(b"dir")).unwrap();
        assert_eq!(listed.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>(), ["free.txt"], "{ext}");
    }
}

#[test]
fn os_error_codes_map_on_this_system() {
    let missing = std::fs::File::open(std::env::temp_dir().join("flit-archive-no-such-file")).unwrap_err();
    assert_eq!(fs_error(io_error(missing)).kind, FsErrorKind::NotFound);
    assert_eq!(fs_error(PlatformError::new(PASSWORD_CODE, "x")).kind, FsErrorKind::AccessDenied);
    assert_eq!(fs_error(PlatformError::new(-1, "x")).kind, FsErrorKind::Other);
    #[cfg(unix)]
    assert_eq!(fs_error(PlatformError::new(13, "x")).kind, FsErrorKind::AccessDenied);
    #[cfg(windows)]
    assert_eq!(fs_error(PlatformError::new(5, "x")).kind, FsErrorKind::AccessDenied);
}

#[cfg(unix)]
#[test]
fn std_path_keeps_non_utf8_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let p = FsPath::parse_bytes(b"/tmp/caf\xe9").unwrap();
    assert_eq!(std_path(&p).as_os_str().as_bytes(), b"/tmp/caf\xe9");
}

fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut crc, &b| {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
        crc
    })
}

fn lh0(name: &str, data: &[u8], crc: u16) -> Vec<u8> {
    let mut h = b"-lh0-".to_vec();
    h.extend((data.len() as u32).to_le_bytes());
    h.extend((data.len() as u32).to_le_bytes());
    h.extend(0x5A21_6000u32.to_le_bytes());
    h.extend([0x20, 0, name.len() as u8]);
    h.extend(name.as_bytes());
    h.extend(crc.to_le_bytes());
    let sum = h.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    let mut out = vec![h.len() as u8, sum];
    out.extend(h);
    out.extend(data);
    out
}

#[test]
fn lzh_lists_as_a_folder_and_extracts_with_a_crc_check() {
    let t = Temp::new("lzh");
    let mut lzh = lh0("docs\\note.txt", b"stored in lha", crc16(b"stored in lha"));
    lzh.extend(lh0("top.bin", b"0123456789", crc16(b"0123456789")));
    lzh.push(0);
    t.write("a.lzh", &lzh);
    assert_eq!(flit_core::archive::browsable(b"a.lzh"), Some(Format::Lzh));
    assert_eq!(names(&t.fs("a.lzh")).unwrap(), vec![("docs".to_owned(), 13, true), ("top.bin".to_owned(), 10, false)]);
    let out = extract(&t.fs("a.lzh"), &t.fs("out"), Place::Here);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(t.read("out/docs/note.txt"), b"stored in lha");
    assert_eq!(t.read("out/top.bin"), b"0123456789");
    let mut bad = lh0("bad.txt", b"payload", crc16(b"payload") ^ 1);
    bad.push(0);
    t.write("bad.lzh", &bad);
    assert!(extract(&t.fs("bad.lzh"), &t.fs("bad"), Place::Here).error.is_some());
}

#[test]
fn seven_z_word_size_changes_the_stream_and_still_extracts() {
    let t = Temp::new("word");
    let mut seed = 7u64;
    let text: Vec<u8> = (0..60_000).flat_map(|_| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        format!("word{} ", (seed >> 33) % 300).into_bytes()
    }).collect();
    t.write("src/text.txt", &text);
    let mut sizes = Vec::new();
    for word in [8, 273] {
        let archive = t.fs(&format!("w{word}.7z"));
        let options = Compression { format: Format::SevenZ, level: 9, word, threads: 1, ..Compression::default() };
        let out = run(FileOp::Archive(ArchiveOp::Compress { items: vec![t.fs("src/text.txt")], archive: archive.clone(), options }), Resolve::Replace);
        assert!(out.error.is_none(), "{:?}", out.error);
        assert!(extract(&archive, &t.fs(&format!("out{word}")), Place::Here).error.is_none());
        assert_eq!(t.read(&format!("out{word}/text.txt")), t.read("src/text.txt"));
        sizes.push(fs::metadata(t.path(&format!("w{word}.7z"))).unwrap().len());
    }
    assert_ne!(sizes[0], sizes[1]);
}

#[test]
fn package_formats_browse_like_zip_and_documents_open_as_folder_only() {
    for name in ["app.jar", "app.apk", "app.msix", "app.appx", "lib.nupkg", "ext.vsix", "x.LHA"] {
        assert!(flit_core::archive::browsable(name.as_bytes()).is_some(), "{name}");
    }
    for name in ["a.docx", "b.epub", "c.odt"] {
        assert!(flit_core::archive::browsable(name.as_bytes()).is_none(), "{name}");
        assert_eq!(flit_core::archive::format_of(name.as_bytes()), Some(Format::Zip), "{name}");
    }
}