use crate::context_menu::Pidls;
use crate::fs::NtFs;
use crate::shell::ShellCom;
use crate::watcher::DirWatcher;
use flit_core::listing::ListingBuilder;
use flit_core::provider::{FileOp, GenToken, Template};
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

fn run(sh: &mut ShellCom, op: &FileOp) -> flit_platform::OpOutcome {
    sh.file_op(op, &flit_platform::Job::detached(flit_core::provider::Resolve::KeepBoth))
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
    measure("copy 1 file 4 KB same volume", RUNS, |k| c.file(&format!("c{k}.txt"), 4096), |f| ok(run(&mut sh, &FileOp::Copy { items: vec![f], dest: c.path("dst") })));
    measure("copy 1 file 4 KB other volume", RUNS, |k| c.file(&format!("x{k}.txt"), 4096), |f| ok(run(&mut sh, &FileOp::Copy { items: vec![f], dest: other.path("dst") })));
    measure("duplicate 1 file in place", RUNS, |k| c.file(&format!("dup{k}.txt"), 4096), |f| ok(run(&mut sh, &FileOp::Copy { items: vec![f], dest: c.path("") })));
    measure("copy 20 files 4 KB", 5, |k| (0..20).map(|i| c.file(&format!("m{k}_{i}.txt"), 4096)).collect::<Vec<_>>(), |items| ok(run(&mut sh, &FileOp::Copy { items, dest: c.path("dst") })));
    measure("copy folder 50 files", 5, |k| {
        std::fs::create_dir(c.0.join(format!("tree{k}"))).unwrap();
        (0..50).for_each(|i| drop(c.file(&format!("tree{k}/f{i}"), 1024)));
        c.path(&format!("tree{k}"))
    }, |f| ok(run(&mut sh, &FileOp::Copy { items: vec![f], dest: c.path("dst") })));
    measure("move 1 file same volume", RUNS, |k| c.file(&format!("mv{k}.txt"), 4096), |f| ok(run(&mut sh, &FileOp::Move { items: vec![f], dest: c.path("dst") })));
    measure("move 1 file other volume", RUNS, |k| c.file(&format!("xv{k}.txt"), 4096), |f| ok(run(&mut sh, &FileOp::Move { items: vec![f], dest: other.path("dst") })));
    measure("rename 1 file", RUNS, |k| c.file(&format!("rn{k}.txt"), 10), |f| ok(run(&mut sh, &FileOp::Rename { item: f, name: format!("renamed{}.txt", rand_tag()).into_bytes() })));
    measure("new folder", RUNS, |k| format!("nf{k}"), |n| ok(run(&mut sh, &FileOp::NewFolder { dir: c.path(""), name: n.into_bytes() })));
    measure("shortcut 1 file", RUNS, |k| c.file(&format!("ln{k}.txt"), 10), |f| ok(run(&mut sh, &FileOp::Link { items: vec![f], dest: c.path("dst") })));
    let mut recycled = Vec::new();
    measure("trash 1 file", RUNS, |k| c.file(&format!("tr{k}.txt"), 4096), |f| {
        let out = run(&mut sh, &FileOp::Trash(vec![f]));
        assert!(out.error.is_none(), "{:?}", out.error);
        recycled.extend(out.items);
    });
    measure("trash 20 files", 3, |k| (0..20).map(|i| c.file(&format!("t{k}_{i}.txt"), 4096)).collect::<Vec<_>>(), |items| {
        let out = run(&mut sh, &FileOp::Trash(items));
        assert!(out.error.is_none(), "{:?}", out.error);
        recycled.extend(out.items);
    });
    let bin = recycle_bin_len();
    eprintln!("      recycle bin holds {bin} items");
    let pairs: Vec<(FsPath, FsPath)> = recycled.iter().map(|(from, to)| (to.clone(), from.clone())).collect();
    measure("restore 1 file from recycle bin", pairs.len(), |k| pairs[k].clone(), |p| ok(run(&mut sh, &FileOp::Restore(vec![p]))));
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

#[test]
#[ignore]
fn bench_new_items() {
    crate::com_sta();
    measure("templates() HKCR ShellNew scan", RUNS, |_| (), |_| {
        assert!(crate::places::templates().iter().all(|t| t.ext.starts_with('.') && !t.label.is_empty() && !t.stem.is_empty()));
    });
    let found: Vec<String> = crate::places::templates().iter().map(|t| format!("{} ({}, {})", t.label, t.stem, t.ext)).collect();
    eprintln!("      {} templates: {}", found.len(), found.join("; "));
    let d = Dir::temp("new");
    let mut sh = ShellCom::new().unwrap();
    let template = d.file("template.docx", 20_000);
    let new = |name: String, template| FileOp::NewFile { dir: d.path(""), name: name.into_bytes(), template };
    measure("new empty file", RUNS, |k| new(format!("e{k}.txt"), Template::Empty), |op| ok(run(&mut sh, &op)));
    measure("new file from 22-byte data", RUNS, |k| new(format!("z{k}.zip"), Template::Data(vec![0x50; 22])), |op| ok(run(&mut sh, &op)));
    measure("new file from 20 KB template file", RUNS, |k| new(format!("t{k}.docx"), Template::Copy(template.clone())), |op| ok(run(&mut sh, &op)));
    measure("new alternate data stream", RUNS, |k| d.file(&format!("s{k}.txt"), 10), |f| ok(run(&mut sh, &FileOp::NewStream { item: f, name: b"meta".to_vec() })));
    std::fs::create_dir(d.0.join("target")).unwrap();
    let links = run(&mut sh, &FileOp::Link { items: vec![d.path("target"), template.clone()], dest: d.path("") }).items;
    let (to_folder, to_file) = (links[0].1.clone(), links[1].1.clone());
    measure(".lnk to a folder resolved", RUNS, |_| to_folder.clone(), |l| assert_eq!(sh.shortcut_folder(&l), Some(d.path("target"))));
    measure(".lnk to a file resolved", RUNS, |_| to_file.clone(), |l| assert_eq!(sh.shortcut_folder(&l), None));
    for n in [100, 1000] {
        let names: Vec<Vec<u8>> = (0..n).map(|i| format!("photo{i}.jpg").into_bytes()).collect();
        let items: Vec<(&[u8], bool)> = names.iter().map(|n| (n.as_slice(), false)).collect();
        measure(&format!("batch_names for {n} items"), RUNS, |_| (), |_| assert_eq!(flit_core::ui::batch_names("Trip", &items, |_| false).len(), n));
        measure(&format!("batch rename {n} files"), 5, |k| {
            let dir = format!("batch{n}_{k}");
            std::fs::create_dir(d.0.join(&dir)).unwrap();
            let new_names = flit_core::ui::batch_names("Trip", &items, |_| false);
            names.iter().zip(new_names).map(|(old, new)| (d.file(&format!("{dir}/{}", String::from_utf8_lossy(old)), 10), d.path(&format!("{dir}/{}", String::from_utf8_lossy(&new))))).collect::<Vec<_>>()
        }, |pairs| ok(run(&mut sh, &FileOp::MoveTo(pairs))));
    }
}

fn tree(d: &Dir, rel: &str, files: usize, bytes: usize) -> FsPath {
    std::fs::create_dir_all(d.0.join(rel)).unwrap();
    (0..files).for_each(|i| drop(d.file(&format!("{rel}/f{i:04}.txt"), bytes)));
    d.path(rel)
}

fn big_file(d: &Dir, rel: &str, mb: usize) -> FsPath {
    use std::io::Write;
    let mut f = std::fs::File::create(d.0.join(rel)).unwrap();
    let chunk: Vec<u8> = (0..8 << 20).map(|i| (i * 31 % 251) as u8).collect();
    (0..mb / 8).for_each(|_| f.write_all(&chunk).unwrap());
    d.path(rel)
}

fn fresh(d: &Dir, rel: &str) -> FsPath {
    let _ = std::fs::remove_dir_all(d.0.join(rel));
    std::fs::create_dir_all(d.0.join(rel)).unwrap();
    d.path(rel)
}

#[test]
#[ignore]
fn bench_engine() {
    let c = Dir::temp("engine");
    let other = Dir::other_volume("engine");
    let mut sh = ShellCom::new().unwrap();
    let mut go = |op: FileOp| ok(run(&mut sh, &op));
    let folder50 = tree(&c, "src50", 50, 1024);
    measure("copy folder of 50 files 1 KB", 5, |k| (folder50.clone(), fresh(&c, &format!("d50_{k}"))), |(f, dest)| go(FileOp::Copy { items: vec![f], dest }));
    let folder1000 = tree(&c, "src1000", 1000, 1024);
    measure("copy folder of 1000 files 1 KB", 5, |k| (folder1000.clone(), fresh(&c, &format!("d1000_{k}"))), |(f, dest)| go(FileOp::Copy { items: vec![f], dest }));
    let loose: Vec<FsPath> = (0..1000).map(|i| c.file(&format!("src1000/f{i:04}.txt"), 1024)).collect();
    measure("copy 1000 selected files 1 KB", 5, |k| (loose.clone(), fresh(&c, &format!("l1000_{k}"))), |(items, dest)| go(FileOp::Copy { items, dest }));
    let big = big_file(&c, "big.bin", 512);
    measure("copy 1 file 512 MB", 3, |_| fresh(&c, "bigdest"), |dest| go(FileOp::Copy { items: vec![big.clone()], dest }));
    let _ = std::fs::remove_dir_all(c.0.join("bigdest"));
    let _ = std::fs::remove_file(c.0.join("big.bin"));
    measure("move folder of 50 files same volume", 5, |k| (tree(&c, &format!("mv50_{k}"), 50, 1024), fresh(&c, &format!("mvd50_{k}"))), |(f, dest)| go(FileOp::Move { items: vec![f], dest }));
    if c.0.components().next() != other.0.components().next() {
        measure("move folder of 50 files other volume", 5, |k| (tree(&c, &format!("xv50_{k}"), 50, 1024), fresh(&other, &format!("xvd50_{k}"))), |(f, dest)| go(FileOp::Move { items: vec![f], dest }));
        measure("move folder of 1000 files other volume", 3, |k| (tree(&c, &format!("xv1000_{k}"), 1000, 1024), fresh(&other, &format!("xvd1000_{k}"))), |(f, dest)| go(FileOp::Move { items: vec![f], dest }));
    } else {
        eprintln!("      skipped cross-volume moves: {} and {} share a volume", c.0.display(), other.0.display());
    }
    measure("batch rename 1000 files", 5, |k| {
        let dir = tree(&c, &format!("ren{k}"), 1000, 10);
        (0..1000).map(|i| (dir.join(format!("f{i:04}.txt").as_bytes()), dir.join(format!("Trip ({}).txt", i + 1).as_bytes()))).collect::<Vec<_>>()
    }, |pairs| go(FileOp::MoveTo(pairs)));
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

fn query_menu(menu: IContextMenu) {
    unsafe {
        let h = CreatePopupMenu().unwrap();
        let _ = menu.QueryContextMenu(h, 0, 1, 0x7FFF, CMF_NORMAL);
        let _ = DestroyMenu(h);
    }
}

fn file_menu(f: &FsPath) -> IContextMenu {
    Pidls::parse(std::slice::from_ref(f)).unwrap().ui_object(HWND::default()).unwrap()
}

fn background_menu(dir: &FsPath) -> IContextMenu {
    let folder = Pidls::parse(std::slice::from_ref(dir)).unwrap().folder().unwrap();
    unsafe { folder.CreateViewObject(HWND::default()).unwrap() }
}

#[test]
#[ignore]
fn bench_context_menu_cold_background() {
    crate::com_sta();
    let d = Dir::temp("bg");
    measure("context menu build+query (background)", RUNS, |_| d.path(""), |p| query_menu(background_menu(&p)));
}

#[test]
#[ignore]
fn bench_context_menu_after_prewarm() {
    let d = Dir::temp("warm");
    let files: Vec<FsPath> = (0..RUNS).map(|k| d.file(&format!("f{k}.txt"), 10)).collect();
    let (warm_file, warm_dir) = (d.file("warm.txt", 1), d.path(""));
    let t = Instant::now();
    std::thread::spawn(move || {
        crate::com_sta();
        query_menu(file_menu(&warm_file));
        query_menu(background_menu(&warm_dir));
    })
    .join()
    .unwrap();
    eprintln!("BENCH prewarm on another thread took {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    crate::com_sta();
    measure("context menu (file) after prewarm", RUNS, |k| files[k].clone(), |f| query_menu(file_menu(&f)));
    measure("context menu (background) after prewarm", RUNS, |_| d.path(""), |p| query_menu(background_menu(&p)));
}

#[test]
#[ignore]
fn bench_context_menu_breakdown() {
    use windows::Win32::UI::Shell::{CMF_ASYNCVERBSTATE, CMF_CANRENAME, CMF_ITEMMENU};
    crate::com_sta();
    let d = Dir::temp("brk");
    let files: Vec<FsPath> = (0..RUNS * 4).map(|k| d.file(&format!("f{k}.txt"), 10)).collect();
    query_menu(file_menu(&files[0]));
    measure("GetUIObjectOf(IContextMenu) only", RUNS, |k| files[k].clone(), |f| drop(file_menu(&f)));
    for (n, (label, flags)) in [("query + ASYNCVERBSTATE", CMF_NORMAL | CMF_CANRENAME | CMF_ITEMMENU | CMF_ASYNCVERBSTATE), ("query flit flags", CMF_NORMAL | CMF_CANRENAME | CMF_ITEMMENU), ("query + ASYNCVERBSTATE again", CMF_NORMAL | CMF_CANRENAME | CMF_ITEMMENU | CMF_ASYNCVERBSTATE)].into_iter().enumerate() {
        measure(label, RUNS, |k| file_menu(&files[RUNS * (n + 1) - RUNS + k]), |m| unsafe {
            let h = CreatePopupMenu().unwrap();
            let _ = m.QueryContextMenu(h, 0, 1, 0x7FFF, flags);
            let _ = DestroyMenu(h);
        });
    }
}

#[test]
#[ignore]
fn bench_drag_setup() {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Ole::OleInitialize;
    use windows::Win32::UI::Shell::{IDragSourceHelper, CLSID_DragDropHelper};
    unsafe { OleInitialize(None).unwrap() };
    let d = Dir::temp("drag");
    let files: Vec<FsPath> = (0..RUNS * 2).map(|k| d.file(&format!("f{k}.txt"), 10)).collect();
    let data = |f: &FsPath| -> IDataObject { Pidls::parse(std::slice::from_ref(f)).unwrap().ui_object(HWND::default()).unwrap() };
    measure("drag image InitializeFromWindow", RUNS, |k| data(&files[k]), |obj| unsafe {
        let helper: IDragSourceHelper = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).unwrap();
        let _ = helper.InitializeFromWindow(None, Some(&POINT { x: 0, y: 0 }), &obj);
    });
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    eprintln!("BENCH {label:<52} {:>9.3} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

#[test]
#[ignore]
fn bench_first_drag() {
    use windows::core::w;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Ole::{OleInitialize, DROPEFFECT_MOVE};
    use windows::Win32::UI::Shell::{IDragSourceHelper, IDropTargetHelper, CLSID_DragDropHelper};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW};
    let d = Dir::temp("firstdrag");
    std::fs::create_dir(d.0.join("dst")).unwrap();
    let wallpaper = Path::new(r"C:\Windows\Web\Wallpaper\Windows\img0.jpg");
    let picture = wallpaper.exists().then(|| {
        std::fs::copy(wallpaper, d.0.join("photo.jpg")).unwrap();
        d.path("photo.jpg")
    });
    let files: Vec<FsPath> = (0..4).map(|k| d.file(&format!("f{k}.txt"), 10)).collect();
    timed("OleInitialize", || unsafe { OleInitialize(None).unwrap() });
    let target: IDropTargetHelper = timed("CoCreateInstance(DragDropHelper) first", || unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).unwrap() });
    let hwnd = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("flit-bench"), WS_OVERLAPPEDWINDOW, 0, 0, 200, 200, None, None, None, None).unwrap() };
    let data = |f: &FsPath| -> IDataObject { Pidls::parse(std::slice::from_ref(f)).unwrap().ui_object(hwnd).unwrap() };
    let source_image = |label: &str, obj: &IDataObject| {
        timed(label, || unsafe {
            let helper: IDragSourceHelper = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).unwrap();
            let _ = helper.InitializeFromWindow(Some(hwnd), Some(&POINT { x: 8, y: 8 }), obj);
        })
    };
    let target_enter = |label: &str, obj: &IDataObject| {
        timed(label, || unsafe {
            let _ = target.DragEnter(hwnd, obj, &POINT { x: -4000, y: -4000 }, DROPEFFECT_MOVE);
            let _ = target.DragLeave();
        })
    };
    let first = timed("data object first", || data(&files[0]));
    timed("data object second", || drop(data(&files[1])));
    source_image("drag image (txt) first", &first);
    source_image("drag image (txt) second", &data(&files[1]));
    if let Some(pic) = &picture {
        source_image("drag image (jpg) first", &data(pic));
        source_image("drag image (jpg) second", &data(pic));
    }
    timed("CF_HDROP paths first", || crate::dragdrop::data_paths(&first));
    target_enter("drop target helper DragEnter+Leave first", &first);
    target_enter("drop target helper DragEnter+Leave second", &data(&files[2]));
    let mv = |k: usize| {
        let (item, dest) = (files[k].clone(), d.path("dst"));
        std::thread::spawn(move || {
            let t = Instant::now();
            let mut sh = ShellCom::new().unwrap();
            let ready = t.elapsed();
            ok(run(&mut sh, &FileOp::Move { items: vec![item], dest }));
            (ready, t.elapsed())
        })
        .join()
        .unwrap()
    };
    for label in ["first", "second"] {
        let (ready, all) = mv(if label == "first" { 0 } else { 1 });
        eprintln!("BENCH engine move {label:<41} {:>9.3} ms (ShellCom::new {:.3} ms)", all.as_secs_f64() * 1000.0, ready.as_secs_f64() * 1000.0);
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[test]
#[ignore]
fn bench_recycle_bin_listing() {
    use crate::recycle::tests::{info_bytes, FakeBin};
    for n in [1_000, 10_000] {
        let bin = FakeBin::new(&format!("bench{n}"));
        for k in 0..n {
            std::fs::write(bin.root.join(format!("$R{k:06}.txt")), b"x").unwrap();
            std::fs::write(bin.root.join(format!("$I{k:06}.txt")), info_bytes(2, 1, 1, &format!(r"C:\Users\me\Documents\file {k}.txt"))).unwrap();
        }
        let roots = [bin.path()];
        measure(&format!("read_dir bin root ({} entries)", 2 * n), RUNS, |_| (), |_| assert_eq!(count(&NtFs, &roots[0]), 2 * n));
        measure(&format!("recycle bin list {n} items"), RUNS, |_| (), |_| assert_eq!(crate::recycle::count(&roots), n));
    }
}

fn thumbnails_of(paths: &[FsPath], threads: usize, size: u32) -> (Duration, usize) {
    let t = Instant::now();
    let chunks: Vec<Vec<FsPath>> = (0..threads).map(|k| paths.iter().skip(k).step_by(threads).cloned().collect()).collect();
    let made: usize = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                s.spawn(move || {
                    let mut sh = ShellCom::new().unwrap();
                    chunk.into_iter().filter(|p| sh.thumbnail(&IconRequest { key: 0, kind: IconKind::Thumbnail, path: p.clone(), size, gen: 0, row: 0 }).is_some()).count()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });
    (t.elapsed(), made)
}

#[test]
#[ignore]
fn bench_thumbnails() {
    const N: usize = 300;
    let d = Dir::temp("thumbs");
    let sets: Vec<Vec<FsPath>> = ["a", "b"]
        .iter()
        .map(|set| {
            (0..N)
                .map(|k| {
                    let name = format!("{set}_{k:04}.jpg");
                    crate::shell::tests::write_jpeg(&d.0.join(&name), 320, 240, k as u32 + if *set == "a" { 0 } else { 10_000 });
                    d.path(&name)
                })
                .collect()
        })
        .collect();
    let bytes: u64 = std::fs::read_dir(&d.0).unwrap().filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum();
    eprintln!("      {} generated JPEG 320x240, {:.1} MB", 2 * N, bytes as f64 / 1e6);
    let rate = |label: &str, (t, made): (Duration, usize)| {
        eprintln!("BENCH {label:<44} {made} in {:>8.1} ms   {:>7.0} per s   {:>6.2} ms each", t.as_secs_f64() * 1e3, made as f64 / t.as_secs_f64(), t.as_secs_f64() * 1e3 / made.max(1) as f64);
    };
    let half = N / 2;
    for (size, part) in [(128, 0), (256, half)] {
        let (a, b) = (&sets[0][part..part + half], &sets[1][part..part + half]);
        rate(&format!("thumbnail {size} first time, 1 thread"), thumbnails_of(a, 1, size));
        rate(&format!("thumbnail {size} first time, 4 threads"), thumbnails_of(b, 4, size));
        rate(&format!("thumbnail {size} cached, 1 thread"), thumbnails_of(a, 1, size));
        rate(&format!("thumbnail {size} cached, 4 threads"), thumbnails_of(b, 4, size));
    }
    let mut sh = ShellCom::new().unwrap();
    let cache = d.0.join("raw");
    std::fs::create_dir_all(&cache).unwrap();
    let raws: Vec<PathBuf> = sets[0][half..]
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let b = sh.thumbnail(&IconRequest { key: 0, kind: IconKind::Thumbnail, path: p.clone(), size: 256, gen: 0, row: 0 }).unwrap();
            let f = cache.join(format!("{k}.raw"));
            std::fs::write(&f, &b.rgba).unwrap();
            f
        })
        .collect();
    let t = Instant::now();
    let total: usize = raws.iter().map(|f| std::fs::read(f).unwrap().len()).sum();
    rate("own raw cache read 256 (warm OS cache)", (t.elapsed(), raws.len()));
    eprintln!("      own raw cache: {:.1} KB per thumbnail", total as f64 / raws.len() as f64 / 1e3);
}
