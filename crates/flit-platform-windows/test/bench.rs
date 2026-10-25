use crate::context_menu::Pidls;
use crate::fs::NtFs;
use crate::shell::ShellCom;
use crate::watcher::DirWatcher;
use flit_core::listing::ListingBuilder;
use flit_core::provider::{FileOp, GenToken};
use flit_core::sort::CaseMode;
use flit_core::ui::{IconKind, IconRequest};
use flit_core::{FsPath, ProviderEvent, ProviderRequest};
use flit_platform::{FileOps, Fs, LocalProvider, Provider, Shell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::UI::Shell::{IContextMenu, CMF_NORMAL};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu};

const RUNS: usize = 15;

struct Dir(PathBuf);

impl Dir {
    fn new(root: &Path, tag: &str) -> Self {
        let dir = root.join(format!("flit-bench-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn temp(tag: &str) -> Self {
        Self::new(&std::env::temp_dir(), tag)
    }
    fn other_volume(tag: &str) -> Self {
        Self::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").as_path(), tag)
    }
    fn path(&self, rel: &str) -> FsPath {
        fs_path(&self.0.join(rel))
    }
    fn file(&self, rel: &str, bytes: usize) -> FsPath {
        std::fs::write(self.0.join(rel), vec![b'x'; bytes]).unwrap();
        self.path(rel)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse(p.to_str().unwrap().trim_end_matches('\\')).unwrap()
}

fn report(label: &str, mut times: Vec<Duration>) {
    let first = times[0];
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    eprintln!("BENCH {label:<44} first {:>9.3} ms   median {:>9.3} ms   max {:>9.3} ms", ms(first), ms(times[times.len() / 2]), ms(times[times.len() - 1]));
}

fn measure<S>(label: &str, runs: usize, mut setup: impl FnMut(usize) -> S, mut op: impl FnMut(S)) {
    let times = (0..runs)
        .map(|k| {
            let s = setup(k);
            let t = Instant::now();
            op(s);
            t.elapsed()
        })
        .collect();
    report(label, times);
}

fn ok(out: flit_platform::OpOutcome) {
    assert!(out.error.is_none(), "{:?}", out.error);
}

fn count(fs: &NtFs, dir: &FsPath) -> usize {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    fs.read_dir(dir, &mut b).unwrap();
    b.listing.len()
}

#[test]
#[ignore]
fn bench_listing() {
    let d = Dir::temp("list");
    for i in 0..10_000 {
        std::fs::write(d.0.join(format!("file_{i:05}.txt")), b"").unwrap();
    }
    let dir = d.path("");
    measure("read_dir 10k files", RUNS, |_| (), |_| assert_eq!(count(&NtFs, &dir), 10_000));
    let sys = FsPath::parse(r"C:\Windows\System32").unwrap();
    measure("read_dir C:\\Windows\\System32", RUNS, |_| (), |_| assert!(count(&NtFs, &sys) > 1000));
    measure("read_dir This PC", RUNS, |_| (), |_| assert!(count(&NtFs, &FsPath::computer()) > 0));
}

fn dir_sizes(provider: &LocalProvider, events: &crossbeam_channel::Receiver<ProviderEvent>, dir: &FsPath, gen: u64, cancel: &GenToken) -> u64 {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    NtFs.read_dir(dir, &mut b).unwrap();
    let items: Vec<(u32, Vec<u8>)> = (0..b.listing.len() as u32).filter(|&i| b.listing.entries[i as usize].is_dir()).map(|i| (i, b.listing.name(i).to_vec())).collect();
    let n = items.len();
    cancel.set(gen);
    provider.submit(ProviderRequest::DirSizes { tab: 1, gen, dir: dir.clone(), items, cancel: cancel.clone() });
    let mut total = 0;
    let mut got = 0;
    while got < n {
        if let ProviderEvent::DirSize { size, .. } = events.recv_timeout(Duration::from_secs(120)).unwrap() {
            total += size;
            got += 1;
        }
    }
    total
}

#[test]
#[ignore]
fn bench_dir_sizes() {
    let d = Dir::temp("sizes");
    for a in 0..20 {
        for b in 0..20 {
            let sub = d.0.join(format!("a{a:02}")).join(format!("b{b:02}"));
            std::fs::create_dir_all(&sub).unwrap();
            for f in 0..25 {
                std::fs::write(sub.join(format!("f{f:02}")), b"0123456789").unwrap();
            }
        }
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    let provider = LocalProvider::new::<_, DirWatcher, ShellCom>(NtFs, 2, tx, Arc::new(|| {})).unwrap();
    let cancel = GenToken::default();
    let dir = d.path("");
    let mut gen = 0;
    measure("dir sizes 400 dirs / 10k files", 5, |_| (), |_| {
        gen += 1;
        assert_eq!(dir_sizes(&provider, &rx, &dir, gen, &cancel), 100_000);
    });
    for big in [r"C:\Windows", r"C:\Program Files"] {
        let p = FsPath::parse(big).unwrap();
        let mut total = 0;
        measure(&format!("dir sizes {big}"), 3, |_| (), |_| {
            gen += 1;
            total = dir_sizes(&provider, &rx, &p, gen, &cancel);
        });
        eprintln!("      {big}: {:.1} GB", total as f64 / 1e9);
    }
}

#[test]
#[ignore]
fn bench_shell_metadata() {
    let d = Dir::temp("meta");
    let mut sh = ShellCom::new().unwrap();
    let req = |kind, path: FsPath| IconRequest { key: 0, kind, path, size: 16, gen: 0, row: 0 };
    let exts = ["txt", "rs", "pdf", "zip", "png", "docx", "mp4", "json", "exe", "dll", "html", "md", "toml", "ps1", "jpg"];
    measure("type_name (per extension)", exts.len(), |k| exts[k].as_bytes(), |e| {
        sh.type_name(e);
    });
    measure("icon by type (per extension)", exts.len(), |k| req(IconKind::Type, d.path(&format!("x.{}", exts[k]))), |r| {
        assert!(sh.icon(&r).is_some());
    });
    measure("icon folder", RUNS, |_| req(IconKind::Folder, d.path("")), |r| {
        assert!(sh.icon(&r).is_some());
    });
    let exes: Vec<FsPath> = std::fs::read_dir(r"C:\Windows\System32").unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "exe")).take(RUNS).map(|p| fs_path(&p)).collect();
    measure("icon per file (exe, distinct)", exes.len(), |k| req(IconKind::File, exes[k].clone()), |r| {
        assert!(sh.icon(&r).is_some());
    });
    measure("icon volume C:", RUNS, |_| req(IconKind::Volume, FsPath::parse(r"C:\").unwrap()), |r| {
        assert!(sh.icon(&r).is_some());
    });
}

#[test]
#[ignore]
fn bench_file_ops() {
    let c = Dir::temp("ops");
    let other = Dir::other_volume("ops");
    let mut sh = ShellCom::new().unwrap();
    std::fs::create_dir(c.0.join("dst")).unwrap();
    std::fs::create_dir(other.0.join("dst")).unwrap();
    measure("copy 1 file 4 KB same volume", RUNS, |k| c.file(&format!("c{k}.txt"), 4096), |f| ok(sh.file_op(&FileOp::Copy { items: vec![f], dest: c.path("dst") })));
    measure("copy 1 file 4 KB other volume", RUNS, |k| c.file(&format!("x{k}.txt"), 4096), |f| ok(sh.file_op(&FileOp::Copy { items: vec![f], dest: other.path("dst") })));
    measure("duplicate 1 file in place", RUNS, |k| c.file(&format!("dup{k}.txt"), 4096), |f| ok(sh.file_op(&FileOp::Copy { items: vec![f], dest: c.path("") })));
    measure("copy 20 files 4 KB", 5, |k| (0..20).map(|i| c.file(&format!("m{k}_{i}.txt"), 4096)).collect::<Vec<_>>(), |items| ok(sh.file_op(&FileOp::Copy { items, dest: c.path("dst") })));
    measure("copy folder 50 files", 5, |k| {
        std::fs::create_dir(c.0.join(format!("tree{k}"))).unwrap();
        (0..50).for_each(|i| drop(c.file(&format!("tree{k}/f{i}"), 1024)));
        c.path(&format!("tree{k}"))
    }, |f| ok(sh.file_op(&FileOp::Copy { items: vec![f], dest: c.path("dst") })));
    measure("move 1 file same volume", RUNS, |k| c.file(&format!("mv{k}.txt"), 4096), |f| ok(sh.file_op(&FileOp::Move { items: vec![f], dest: c.path("dst") })));
    measure("move 1 file other volume", RUNS, |k| c.file(&format!("xv{k}.txt"), 4096), |f| ok(sh.file_op(&FileOp::Move { items: vec![f], dest: other.path("dst") })));
    measure("rename 1 file", RUNS, |k| c.file(&format!("rn{k}.txt"), 10), |f| ok(sh.file_op(&FileOp::Rename { item: f, name: format!("renamed{}.txt", rand_tag()).into_bytes() })));
    measure("new folder", RUNS, |k| format!("nf{k}"), |n| ok(sh.file_op(&FileOp::NewFolder { dir: c.path(""), name: n.into_bytes() })));
    measure("shortcut 1 file", RUNS, |k| c.file(&format!("ln{k}.txt"), 10), |f| ok(sh.file_op(&FileOp::Link { items: vec![f], dest: c.path("dst") })));
    let mut recycled = Vec::new();
    measure("trash 1 file", RUNS, |k| c.file(&format!("tr{k}.txt"), 4096), |f| {
        let out = sh.file_op(&FileOp::Trash(vec![f]));
        assert!(out.error.is_none(), "{:?}", out.error);
        recycled.extend(out.items);
    });
    measure("trash 20 files", 3, |k| (0..20).map(|i| c.file(&format!("t{k}_{i}.txt"), 4096)).collect::<Vec<_>>(), |items| {
        let out = sh.file_op(&FileOp::Trash(items));
        assert!(out.error.is_none(), "{:?}", out.error);
    });
    let bin = recycle_bin_len();
    eprintln!("      recycle bin holds {bin} items");
    let pairs: Vec<(FsPath, FsPath)> = recycled.iter().map(|(from, to)| (to.clone(), from.clone())).collect();
    measure("restore 1 file from recycle bin", pairs.len(), |k| pairs[k].clone(), |p| ok(sh.file_op(&FileOp::Restore(vec![p]))));
}

#[test]
#[ignore]
fn bench_shell_ui_objects() {
    crate::com_sta();
    let d = Dir::temp("ui");
    let files: Vec<FsPath> = (0..RUNS).map(|k| d.file(&format!("f{k}.txt"), 10)).collect();
    measure("context menu build+query (file)", RUNS, |k| files[k].clone(), |f| unsafe {
        let menu: IContextMenu = Pidls::parse(&[f]).unwrap().ui_object(HWND::default()).unwrap();
        let h = CreatePopupMenu().unwrap();
        let _ = menu.QueryContextMenu(h, 0, 1, 0x7FFF, CMF_NORMAL);
        let _ = DestroyMenu(h);
    });
    measure("drag data object (1 file)", RUNS, |k| files[k].clone(), |f| {
        let _: IDataObject = Pidls::parse(&[f]).unwrap().ui_object(HWND::default()).unwrap();
    });
}

#[test]
#[ignore]
fn bench_places() {
    measure("drives()", RUNS, |_| (), |_| assert!(!crate::places::drives().is_empty()));
    measure("wsl_distros()", RUNS, |_| (), |_| drop(crate::places::wsl_distros()));
    measure("known_folder shell:Downloads", RUNS, |_| (), |_| assert!(<crate::Windows as flit_platform::Platform>::known_folder("shell:Downloads").is_some()));
}

fn rand_tag() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

fn recycle_bin_len() -> usize {
    use windows::Win32::UI::Shell::{IEnumShellItems, IShellItem, SHGetKnownFolderItem, BHID_EnumItems, FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT};
    unsafe {
        let bin: IShellItem = SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None).unwrap();
        let items: IEnumShellItems = bin.BindToHandler(None, &BHID_EnumItems).unwrap();
        let mut n = 0;
        loop {
            let mut one = [None];
            let mut got = 0;
            if items.Next(&mut one, Some(&mut got)).is_err() || got == 0 {
                return n;
            }
            n += 1;
        }
    }
}
