use super::*;
use crate::fs::NtFs;
use crate::watcher::DirWatcher;
use crossbeam_channel::{unbounded, Receiver};
use flit_core::listing::ListingBuilder;
use flit_core::provider::{FileOp, GenToken, OpCtl, Resolve, SnapshotSlot};
use flit_core::sort::{CaseMode, ViewSpec};
use flit_core::{FsPath, ProviderEvent, ProviderRequest};
use flit_platform::{Clipboard, Fs, Job, Provider, Timing, Watcher, WslProvider};
use flit_proto::transport::{read_hello, write_frame, Hello, Protocol};
use std::sync::{Arc, Mutex};

const RUNS: usize = 10;

fn distro() -> String {
    std::env::var("FLIT_TEST_DISTRO").unwrap_or_else(|_| "Ubuntu-22.04".into())
}

fn image() -> AgentImage {
    let path = std::env::var_os("FLIT_WSL_AGENT_X64").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wsl-agent/x86_64-unknown-linux-musl/wsl-agent/flit-wsl-agent"), PathBuf::from);
    let bytes: &'static [u8] = Box::leak(std::fs::read(&path).unwrap_or_else(|e| panic!("build the agent first ({}): {e}", path.display())).into_boxed_slice());
    AgentImage { bytes, build: Box::leak(flit_proto::build_id::build_id(bytes).into_boxed_str()) }
}

fn win(p: &FsPath) -> PathBuf {
    PathBuf::from(p.display().into_owned())
}

fn run(script: &str) -> String {
    let out = shell(&distro(), script).unwrap().wait_with_output().unwrap();
    assert!(out.status.success(), "{script}: {}", decode(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

struct LinuxDir(String);

impl LinuxDir {
    fn new(tag: &str, files: usize) -> Self {
        let dir = format!("/tmp/flit-{tag}-{}", std::process::id());
        run(&format!("rm -rf {dir}; mkdir -p {dir}; cd {dir}; i=0; while [ $i -lt {files} ]; do : > file-$i; i=$((i+1)); done"));
        Self(dir)
    }

    fn unc(&self) -> FsPath {
        FsPath::parse(&format!(r"\\wsl.localhost\{}{}", distro(), self.0.replace('/', "\\"))).unwrap()
    }
}

impl Drop for LinuxDir {
    fn drop(&mut self) {
        let _ = shell(&distro(), &format!("rm -rf {}", self.0)).and_then(|c| c.wait_with_output());
    }
}

#[derive(Default)]
struct Nothing(Mutex<Vec<String>>);

impl Provider for Nothing {
    fn submit(&self, req: ProviderRequest) {
        self.0.lock().unwrap().push(format!("{req:?}"));
    }
}

fn provider() -> (WslProvider, Receiver<ProviderEvent>, Arc<Nothing>) {
    let (tx, rx) = unbounded();
    let local = Arc::new(Nothing::default());
    let p = WslProvider::new(distro(), Some(Box::new(WslLauncher::new(&distro(), image()))), local.clone(), tx, Arc::new(|| {}), Timing::default(), Clipboard::default()).unwrap();
    (p, rx, local)
}

fn agent_op(p: &WslProvider, rx: &Receiver<ProviderEvent>, op: FileOp) -> (Duration, Vec<(FsPath, FsPath)>) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let t = Instant::now();
    p.submit(ProviderRequest::Op { tab: 1, id, op, ctl: OpCtl::new(Some(Resolve::Replace)) });
    loop {
        if let ProviderEvent::OpDone { id: done, error, report, .. } = rx.recv_timeout(Duration::from_secs(120)).expect("agent op") {
            if done == id {
                assert_eq!(error, None);
                return (t.elapsed(), report.map(|r| r.items).unwrap_or_default());
            }
        }
    }
}

fn share_op(op: FileOp) -> Duration {
    let t = Instant::now();
    let out = crate::engine::run(&op, &Job::detached(Resolve::Replace));
    assert!(out.error.is_none() && out.rest.is_none(), "{:?}", out.error.map(|e| e.message));
    t.elapsed()
}

fn ux(p: &str) -> FsPath {
    FsPath::parse(p).unwrap()
}

fn unc(p: &str) -> FsPath {
    FsPath::parse(&format!(r"\\wsl.localhost\{}{}", distro(), p.replace('/', "\\"))).unwrap()
}

#[test]
#[ignore]
fn real_agent_copies_renames_and_deletes_names_windows_cannot_hold() {
    let dir = LinuxDir::new("ops-e2e", 0);
    run(&format!(r"cd {0} && mkdir src && printf x > 'src/a:b' && printf y > src/$(printf 'bad\377') && ln -s /etc src/link && mkdir dst", dir.0));
    let (p, rx, local) = provider();
    let at = |rel: &str| ux(&format!("{}/{rel}", dir.0));
    agent_op(&p, &rx, FileOp::Copy { items: vec![at("src")], dest: at("dst") });
    assert_eq!(run(&format!(r"cd {}/dst/src && ls -A | wc -l && readlink link", dir.0)).split_whitespace().collect::<Vec<_>>(), ["3", "/etc"]);
    agent_op(&p, &rx, FileOp::Rename { item: at("dst/src/a:b"), name: b"c?d".to_vec() });
    assert_eq!(run(&format!(r"cat '{}/dst/src/c?d'", dir.0)), "x");
    agent_op(&p, &rx, FileOp::Delete(vec![at("dst/src")]));
    assert_eq!(run(&format!("ls -A {}/dst | wc -l; test -e /etc/hostname && echo kept", dir.0)).split_whitespace().collect::<Vec<_>>(), ["0", "kept"]);
    assert!(local.0.lock().unwrap().is_empty(), "nothing went through \\\\wsl.localhost: {:?}", local.0.lock().unwrap());
}

#[test]
#[ignore]
fn bench_wsl_operations_agent_vs_share() {
    const OPS_RUNS: usize = 3;
    let dir = LinuxDir::new("bench-ops", 0);
    let d = dir.0.clone();
    run(&format!("cd {d} && mkdir small && cd small && i=0; while [ $i -lt 1000 ]; do head -c 1024 /dev/urandom > f-$i; i=$((i+1)); done"));
    run(&format!("head -c 268435456 /dev/urandom > {d}/big"));
    run(&format!("cd {d} && mkdir tenk && cd tenk && i=0; while [ $i -lt 10000 ]; do : > f-$i; i=$((i+1)); done"));
    let (p, rx, local) = provider();
    list(&p, 1, &d, false);
    let mut rows: Vec<(String, Vec<Duration>, Vec<Duration>)> = Vec::new();
    let mut record = |label: &str, agent: Duration, share: Duration| match rows.iter_mut().find(|r| r.0 == label) {
        Some(r) => {
            r.1.push(agent);
            r.2.push(share);
        }
        None => rows.push((label.to_owned(), vec![agent], vec![share])),
    };
    for k in 0..OPS_RUNS {
        let mut timings = [[Duration::ZERO; 2]; 8];
        for (side, agent) in [true, false].into_iter().enumerate() {
            let tag = format!("{k}{}", if agent { "a" } else { "s" });
            run(&format!("mkdir {d}/c{tag} {d}/m{tag} {d}/b{tag} {d}/bm{tag}"));
            let small: Vec<String> = (0..1000).map(|i| format!("{d}/c{tag}/small/f-{i}")).collect();
            let ops = [
                FileOp::Copy { items: vec![ux(&format!("{d}/small"))], dest: ux(&format!("{d}/c{tag}")) },
                FileOp::Move { items: small.iter().map(|s| ux(s)).collect(), dest: ux(&format!("{d}/m{tag}")) },
                FileOp::Delete(vec![ux(&format!("{d}/m{tag}"))]),
                FileOp::Copy { items: vec![ux(&format!("{d}/big"))], dest: ux(&format!("{d}/b{tag}")) },
                FileOp::Move { items: vec![ux(&format!("{d}/b{tag}/big"))], dest: ux(&format!("{d}/bm{tag}")) },
                FileOp::Delete(vec![ux(&format!("{d}/bm{tag}/big"))]),
                FileOp::Rename { item: ux(&format!("{d}/tenk")), name: b"tenk2".to_vec() },
                FileOp::Rename { item: ux(&format!("{d}/tenk2")), name: b"tenk".to_vec() },
            ];
            for (i, op) in ops.into_iter().enumerate() {
                timings[i][side] = if agent { agent_op(&p, &rx, op).0 } else { share_op(op.map_paths(|p| Some(unc(&String::from_utf8_lossy(p.as_bytes())))).unwrap()) };
            }
            assert_eq!(run(&format!("ls {d}/c{tag}/small | wc -l; ls {d}/m{tag} 2>/dev/null | wc -l; ls {d}/bm{tag} | wc -l; ls {d}/tenk | wc -l")).split_whitespace().collect::<Vec<_>>(), ["0", "0", "0", "10000"]);
            run(&format!("rm -rf {d}/c{tag} {d}/b{tag} {d}/bm{tag}"));
        }
        let labels = ["copy 1000 x 1 KB", "move 1000 x 1 KB", "delete 1000 x 1 KB", "copy 256 MB", "move 256 MB", "delete 256 MB", "rename 10k folder", "rename 10k folder back"];
        labels.iter().zip(timings).for_each(|(l, [a, s])| record(l, a, s));
    }
    let ms = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2].as_secs_f64() * 1e3
    };
    for (label, agent, share) in rows {
        eprintln!("BENCH {label:<26} agent {:>10.1} ms   9P {:>10.1} ms", ms(agent), ms(share));
    }
    assert!(local.0.lock().unwrap().is_empty());
}

fn list(p: &WslProvider, gen: u64, dir: &str, watch: bool) -> (usize, Duration) {
    let (slot, token) = (Arc::new(SnapshotSlot::default()), GenToken::default());
    token.set(gen);
    let path = FsPath::parse(dir).unwrap();
    let t = Instant::now();
    if watch {
        p.submit(ProviderRequest::Watch { tab: 1, gen, path: Some(path.clone()) });
    }
    p.submit(ProviderRequest::List { tab: 1, gen, path, view: ViewSpec::default(), case: CaseMode::Sensitive, out: slot.clone(), cancel: token });
    loop {
        if let Some(s) = slot.take().filter(|s| s.done) {
            assert_eq!(s.error, None);
            return (s.listing.len(), t.elapsed());
        }
        assert!(t.elapsed() < Duration::from_secs(30), "no listing");
        std::thread::sleep(Duration::from_micros(200));
    }
}

fn report(label: &str, mut times: Vec<Duration>) {
    let first = times[0];
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    eprintln!("BENCH {label:<44} first {:>9.3} ms   median {:>9.3} ms   max {:>9.3} ms", ms(first), ms(times[times.len() / 2]), ms(times[times.len() - 1]));
}

#[test]
fn decode_reads_wsl_utf16_and_linux_utf8() {
    let utf16: Vec<u8> = "wsl: warning\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(decode(&utf16), "wsl: warning");
    let russian: Vec<u8> = "wsl: прокси localhost\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(decode(&russian), "wsl: прокси localhost");
    assert_eq!(decode("sh: 1: exec: нет\n".as_bytes()), "sh: 1: exec: нет");
    assert_eq!(decode(b""), "");
}

#[test]
#[ignore]
fn wsl_stdio_is_binary_transparent() {
    let mut child = shell(&distro(), "od -An -tx1 -v").unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(&[0, 10, 13, 10, 0xFF, 26]).unwrap();
    drop(input);
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).split_whitespace().collect::<Vec<_>>(), ["00", "0a", "0d", "0a", "ff", "1a"]);
    assert_eq!(run(r#"printf '%s|%s' "$HOME" "${XDG_CACHE_HOME:-$HOME/.cache}""#).split('|').count(), 2);
}

#[test]
#[ignore]
fn real_agent_lists_watches_and_falls_back_nowhere() {
    let dir = LinuxDir::new("e2e", 3);
    run(&format!(r"printf x > '{0}/a:b'; printf y > '{0}'/$(printf 'bad\377')", dir.0));
    let (p, rx, local) = provider();
    assert_eq!(list(&p, 1, &dir.0, true).0, 5);
    std::fs::write(win(&dir.unc().join(b"from-windows")), b"hello").unwrap();
    let changes = std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(5)).ok()).find_map(|e| if let ProviderEvent::Changes { changes, .. } = e { Some(changes) } else { None }).expect("inotify event");
    assert!(changes.iter().any(|c| c.name == b"from-windows" && c.meta.is_some_and(|m| m.size == 5)), "{changes:?}");
    assert!(local.0.lock().unwrap().is_empty(), "nothing went through \\\\wsl.localhost: {:?}", local.0.lock().unwrap());
}

#[test]
#[ignore]
fn killing_wsl_exe_ends_the_agent() {
    let launcher = WslLauncher::new(&distro(), image());
    launcher.deliver().unwrap();
    let ours = Hello { build: launcher.build().into(), ..Hello::new(Protocol::Provider, flit_proto::provider::VERSION) };
    let mut p = launcher.launch().unwrap();
    write_frame(&mut p.input, &ours).unwrap();
    read_hello(&mut p.output, &ours).unwrap();
    let running = || run(&format!("pgrep -f '[f]lit/agent-{}' || true", launcher.build())).trim().to_owned();
    assert!(!running().is_empty());
    (p.stop)(Duration::ZERO);
    let end = Instant::now() + Duration::from_secs(5);
    while !running().is_empty() {
        assert!(Instant::now() < end, "the agent outlived wsl.exe");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn bench_wsl_warm_start() {
    let launcher = WslLauncher::new(&distro(), image());
    let ours = Hello { build: launcher.build().into(), ..Hello::new(Protocol::Provider, flit_proto::provider::VERSION) };
    let start = |deliver: bool| {
        if deliver {
            launcher.deliver().unwrap();
        }
        let t = Instant::now();
        let mut p = launcher.launch().unwrap();
        write_frame(&mut p.input, &ours).unwrap();
        let theirs = read_hello(&mut p.output, &ours).unwrap();
        let took = t.elapsed();
        assert_eq!(theirs.build, launcher.build());
        drop(p.input);
        (p.stop)(Duration::from_secs(1));
        took
    };
    let t = Instant::now();
    launcher.deliver().unwrap();
    eprintln!("BENCH {:<44} {:>9.3} ms", "wsl agent delivery (650 KB)", t.elapsed().as_secs_f64() * 1e3);
    report("wsl agent warm start to Hello", (0..RUNS).map(|_| start(false)).collect());
    let dir = LinuxDir::new("bench-start", 1);
    let t = Instant::now();
    let (p, _, _) = provider();
    list(&p, 1, &dir.0, false);
    eprintln!("BENCH {:<44} {:>9.3} ms", "first listing through a fresh provider", t.elapsed().as_secs_f64() * 1e3);
}

#[test]
#[ignore]
fn bench_wsl_listing_10k() {
    let dir = LinuxDir::new("bench-10k", 10_000);
    let (p, _, _) = provider();
    list(&p, 1, &dir.0, false);
    report("10k entries via agent", (0..RUNS).map(|k| list(&p, k as u64 + 2, &dir.0, false)).inspect(|(n, _)| assert_eq!(*n, 10_000)).map(|(_, t)| t).collect());
    let unc = dir.unc();
    let share = |_| {
        let t = Instant::now();
        let mut keep = |_: &ListingBuilder<'_>| true;
        let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut keep);
        NtFs.read_dir(&unc, &mut b).unwrap();
        assert_eq!(b.listing.len(), 10_000);
        t.elapsed()
    };
    report("10k entries via \\\\wsl.localhost (NtFs)", (0..RUNS).map(share).collect());
}

#[test]
#[ignore]
fn bench_wsl_watch_latency() {
    let dir = LinuxDir::new("bench-watch", 0);
    let (p, rx, _) = provider();
    list(&p, 1, &dir.0, true);
    let unc = dir.unc();
    let times = (0..RUNS)
        .map(|k| {
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(100));
            let name = format!("w{k}");
            std::fs::write(win(&unc.join(name.as_bytes())), b"x").unwrap();
            let t = Instant::now();
            loop {
                if let Ok(ProviderEvent::Changes { changes, .. }) = rx.recv_timeout(Duration::from_secs(5)) {
                    if changes.iter().any(|c| c.name == name.as_bytes()) {
                        return t.elapsed();
                    }
                }
                assert!(t.elapsed() < Duration::from_secs(5), "no change event");
            }
        })
        .collect();
    report("watch latency (9P write -> Changes)", times);
}

#[test]
#[ignore]
fn share_names_and_notifications() {
    let dir = LinuxDir::new("share", 0);
    run(&format!("cd {} && : > 'a:b' && : > 'dot.' && : > 'space ' && : > nul.txt && : > con", dir.0));
    let mut keep = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut keep);
    NtFs.read_dir(&dir.unc(), &mut b).unwrap();
    let mut names: Vec<String> = (0..b.listing.len() as u32).map(|i| String::from_utf8_lossy(b.listing.name(i)).into_owned()).collect();
    names.sort();
    eprintln!("SHARE names {names:?}");
    assert!(names.contains(&"a\u{F03A}b".to_owned()));
    let (tx, rx) = unbounded();
    let mut w = DirWatcher::new(tx, Arc::new(|| {})).unwrap();
    w.watch(1, 1, Some(&dir.unc()));
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(win(&dir.unc().join(b"from-windows")), b"x").unwrap();
    run(&format!(": > {}/from-linux", dir.0));
    let seen: Vec<ProviderEvent> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(2)).ok()).collect();
    eprintln!("SHARE notifications {seen:?}");
}
