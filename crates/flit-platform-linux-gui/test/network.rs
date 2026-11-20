use super::*;
use flit_core::sort::CaseMode;
use flit_core::Listing;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

const SOURCE: &str = "captured 2026-11-21 in WSL Ubuntu 22.04 with gvfs 1.48.2-0ubuntu1.3 (already installed), and avahi-daemon, avahi-utils 0.8-5ubuntu5.5 and samba 4.15.13 installed for these samples; services published with avahi-publish";

const GVFS_FIRST: &str = "smb-root\t0\t(shortcut)\tstandard::display-name=Windows Network standard::target-uri=smb:///\n";

const GVFS_LATER: &str = "dnssd-domain-%D0%A4%D0%B0%D0%B9%D0%BB%D1%8B%20%D0%BD%D0%B0%20%D1%81%D0%B5%D1%80%D0%B2%D0%B5%D1%80%D0%B5._sftp-ssh._tcp\t0\t(shortcut)\tstandard::display-name=Файлы на сервере standard::target-uri=sftp://brano-2.local:22/
dnssd-domain-BRANO._smb._tcp\t0\t(shortcut)\tstandard::display-name=BRANO standard::target-uri=smb://brano-2.local:445/
dnssd-domain-Team%20DAV._webdav._tcp\t0\t(shortcut)\tstandard::display-name=Team DAV standard::target-uri=dav+sd://Team%20DAV._webdav._tcp.local
dnssd-domain-nfs%20export._nfs._tcp\t0\t(shortcut)\tstandard::display-name=nfs export standard::target-uri=nfs://brano-2.local:2049/export
smb-root\t0\t(shortcut)\tstandard::display-name=Windows Network standard::target-uri=smb:///
";

const GVFS_WITH_ICON: &str = "smb-root\t0\t(shortcut)\tstandard::display-name=Windows Network standard::icon=GThemedIcon:0x5a0fbffb0b90 standard::target-uri=smb:///\n";

const SHARES: &str = "print$\t0\t(mountable)\tstandard::display-name=print$ standard::target-uri=smb://brano-2.local/print%24
Общие документы\t0\t(mountable)\tstandard::display-name=Общие документы standard::target-uri=smb://brano-2.local/%D0%9E%D0%B1%D1%89%D0%B8%D0%B5%20%D0%B4%D0%BE%D0%BA%D1%83%D0%BC%D0%B5%D0%BD%D1%82%D1%8B
private\t0\t(mountable)\tstandard::display-name=private standard::target-uri=smb://brano-2.local/private
";

const NOT_MOUNTED: &str = "gio: smb://brano.local/: The specified location is not mounted\n";
const NOT_SUPPORTED: &str = "gio: nosuch:///: The specified location is not supported\n";
const REFUSED: &str = "gio: smb://127.0.0.1/: Failed to retrieve share list from server: Connection refused\n";

const AVAHI: &str = r#"+;eth0;IPv6;\208\164\208\176\208\185\208\187\209\139\032\208\189\208\176\032\209\129\208\181\209\128\208\178\208\181\209\128\208\181;_sftp-ssh._tcp;local
+;eth0;IPv4;Team\032DAV;_webdav._tcp;local
+;eth0;IPv4;BRANO;_device-info._tcp;local
=;eth0;IPv6;\208\164\208\176\208\185\208\187\209\139\032\208\189\208\176\032\209\129\208\181\209\128\208\178\208\181\209\128\208\181;_sftp-ssh._tcp;local;brano-2.local;fe80::215:5dff:fe80:b343;22;
=;eth0;IPv4;\208\164\208\176\208\185\208\187\209\139\032\208\189\208\176\032\209\129\208\181\209\128\208\178\208\181\209\128\208\181;_sftp-ssh._tcp;local;brano-2.local;172.18.196.252;22;
=;lo;IPv4;\208\164\208\176\208\185\208\187\209\139\032\208\189\208\176\032\209\129\208\181\209\128\208\178\208\181\209\128\208\181;_sftp-ssh._tcp;local;brano-2.local;10.255.255.254;22;
=;eth0;IPv6;Team\032DAV;_webdav._tcp;local;brano-2.local;fe80::215:5dff:fe80:b343;8080;"u=alice" "path=/dav"
=;eth0;IPv4;Team\032DAV;_webdav._tcp;local;brano-2.local;172.18.196.252;8080;"u=alice" "path=/dav"
=;eth0;IPv4;BRANO;_device-info._tcp;local;brano-2.local;172.18.196.252;0;"model=MacSamba"
=;eth0;IPv6;BRANO;_smb._tcp;local;brano-2.local;fe80::215:5dff:fe80:b343;445;
=;eth0;IPv4;BRANO;_smb._tcp;local;brano-2.local;172.18.196.252;445;
=;eth0;IPv4;Flit\032\208\191\209\128\208\190\208\178\208\181\209\128\208\186\208\176\032NFS;_nfs._tcp;local;brano-2.local;172.18.196.252;2049;"path=/export"
"#;

fn found(name: &str, uri: &str) -> Found {
    Found { name: name.into(), uri: uri.into() }
}

fn done(code: i32, stdout: &str, stderr: &str) -> io::Result<Output> {
    Ok(Output { status: ExitStatus::from_raw(code << 8), stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() })
}

struct Fake {
    lists: RefCell<VecDeque<io::Result<Output>>>,
    asked: RefCell<Vec<(String, Duration)>>,
    mounted: RefCell<Vec<String>>,
    lines: Option<Vec<String>>,
    browse_error: Cell<Option<io::ErrorKind>>,
    browsed: Cell<Option<Duration>>,
    clock: Cell<Instant>,
    delays: RefCell<VecDeque<Duration>>,
}

impl Fake {
    fn new(lists: Vec<io::Result<Output>>) -> Self {
        Fake { lists: RefCell::new(lists.into()), asked: RefCell::default(), mounted: RefCell::default(), lines: None, browse_error: Cell::new(Some(io::ErrorKind::NotFound)), browsed: Cell::new(None), clock: Cell::new(Instant::now()), delays: RefCell::default() }
    }

    fn avahi(mut self, text: &str) -> Self {
        self.lines = Some(text.lines().map(str::to_owned).collect());
        self.browse_error.set(None);
        self
    }

    fn calls(&self) -> usize {
        self.asked.borrow().len()
    }
}

impl Probe for Fake {
    fn list(&self, uri: &str, wait: Duration) -> io::Result<Output> {
        self.asked.borrow_mut().push((uri.to_owned(), wait));
        let late = self.delays.borrow_mut().pop_front().unwrap_or_default();
        self.clock.set(self.clock.get() + late);
        let next = self.lists.borrow_mut().pop_front();
        next.unwrap_or_else(|| done(0, GVFS_LATER, ""))
    }

    fn mount(&self, uri: &str, _wait: Duration) -> io::Result<Output> {
        self.mounted.borrow_mut().push(uri.to_owned());
        done(0, "", "")
    }

    fn browse(&self, wait: Duration, each: &mut dyn FnMut(Option<&str>) -> bool) -> io::Result<()> {
        self.browsed.set(Some(wait));
        if let Some(kind) = self.browse_error.get() {
            return Err(io::Error::from(kind));
        }
        for line in self.lines.iter().flatten() {
            if !each(Some(line)) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn pause(&self, d: Duration) {
        self.clock.set(self.clock.get() + d);
    }

    fn now(&self) -> Instant {
        self.clock.get()
    }
}

fn run(fake: &Fake, dir: &FsPath, mut keep: impl FnMut(usize) -> bool) -> (Result<(), FsError>, Listing, Vec<usize>) {
    let mut batches = Vec::new();
    let mut go = |b: &ListingBuilder<'_>| {
        batches.push(b.listing.len());
        keep(b.listing.len())
    };
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    let result = read_dir_with(fake, dir, &mut b, 0);
    let listing = std::mem::take(&mut b.listing);
    drop(b);
    (result, listing, batches)
}

fn shown(l: &Listing) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.label(i)).into_owned(), String::from_utf8_lossy(l.name(i)).into_owned())).collect();
    out.sort();
    out
}

fn server(uri: &str) -> FsPath {
    FsPath::network_server(uri.as_bytes()).unwrap()
}

#[test]
fn gvfs_network_lines_become_servers_with_their_addresses() {
    assert_eq!(
        parse_network(GVFS_LATER),
        [
            found("Файлы на сервере", "sftp://brano-2.local:22/"),
            found("BRANO", "smb://brano-2.local:445/"),
            found("Team DAV", "dav+sd://Team%20DAV._webdav._tcp.local"),
            found("nfs export", "nfs://brano-2.local:2049/export"),
        ],
        "{SOURCE}"
    );
    assert!(parse_network(GVFS_FIRST).is_empty(), "Windows Network has no host and cannot be opened as a server");
    assert!(parse_network(GVFS_WITH_ICON).is_empty());
}

#[test]
fn gvfs_output_without_addresses_or_with_junk_gives_nothing() {
    assert!(parse_network("").is_empty());
    assert!(parse_network("garbage\n\t\t\n\u{0}\u{1}\nname\t0\t(shortcut)\tstandard::display-name=x\n").is_empty());
    assert!(parse_network("x\t0\t(shortcut)\tstandard::target-uri=nonsense\n").is_empty());
    assert!(parse_network("x\t0\t(shortcut)\tstandard::target-uri=https://nas/\n").is_empty());
    assert_eq!(parse_network("x\t0\t(shortcut)\tstandard::target-uri=smb://nas/\n"), [found("x", "smb://nas/")], "a missing display name falls back to the file name");
}

#[test]
fn shares_keep_unicode_names_and_hide_administrative_shares() {
    assert_eq!(
        parse_shares(SHARES),
        [found("Общие документы", "smb://brano-2.local/%D0%9E%D0%B1%D1%89%D0%B8%D0%B5%20%D0%B4%D0%BE%D0%BA%D1%83%D0%BC%D0%B5%D0%BD%D1%82%D1%8B"), found("private", "smb://brano-2.local/private")],
        "{SOURCE}"
    );
}

#[test]
fn avahi_resolved_services_become_addresses_once_per_service() {
    let mut out: Vec<Found> = Vec::new();
    for f in AVAHI.lines().filter_map(parse_avahi) {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    assert_eq!(out, [found("Файлы на сервере", "sftp://brano-2.local/"), found("Team DAV", "dav://alice@brano-2.local:8080/dav"), found("BRANO", "smb://brano-2.local/"), found("Flit проверка NFS", "nfs://brano-2.local/export")], "{SOURCE}");
}

#[test]
fn avahi_lines_without_a_host_name_use_the_address_and_junk_is_skipped() {
    assert_eq!(parse_avahi("=;eth0;IPv6;nas;_smb._tcp;local;;fe80::1;445;").unwrap().uri, "smb://[fe80::1]/");
    assert_eq!(parse_avahi("=;eth0;IPv4;nas;_nfs._tcp;local;nas.local;10.0.0.2;2049;\"path=/srv/nfs\"").unwrap().uri, "nfs://nas.local/srv/nfs");
    assert_eq!(parse_avahi("=;eth0;IPv4;my\\059box\\092x;_sftp-ssh._tcp;local;box.local;10.0.0.3;2222;").unwrap(), found("my;box\\x", "sftp://box.local:2222/"));
    assert_eq!(parse_avahi("=;eth0;IPv4;Terminal;_ssh._tcp;local;box.local;10.0.0.3;2222;").unwrap(), found("Terminal", "sftp://box.local:2222/"));
    assert_eq!(parse_avahi("=;eth0;IPv4;dav;_webdav._tcp;local;box.local;10.0.0.3;80;\"path=/общие документы#1\" \"u=a@b\"").unwrap().uri, "dav://a%40b@box.local/%D0%BE%D0%B1%D1%89%D0%B8%D0%B5%20%D0%B4%D0%BE%D0%BA%D1%83%D0%BC%D0%B5%D0%BD%D1%82%D1%8B%231");
    for junk in ["", "=;", "+;eth0;IPv4;BRANO;_smb._tcp;local", "=;eth0;IPv4;x;_smb._tcp;local;;;445;", "garbage;;;;;;;;;;", "=;eth0;IPv4;printer;_ipp._tcp;local;p.local;10.0.0.9;631;"] {
        assert_eq!(parse_avahi(junk), None, "{junk:?}");
    }
}

#[test]
fn smb_servers_are_told_from_shares_and_other_services() {
    for yes in ["smb://nas", "smb://nas/", "SMB://brano-2.local:445/", "smb://ann@nas//"] {
        assert!(smb_server(yes), "{yes}");
    }
    for no in ["smb:///", "smb://nas/docs", "sftp://nas/", "dav+sd://Team%20DAV._webdav._tcp.local", "nas", ""] {
        assert!(!smb_server(no), "{no}");
    }
    assert_eq!(server_place("smb://brano-2.local:445/").unwrap(), server("smb://brano-2.local:445/"));
}

#[test]
fn computers_appear_round_by_round_until_the_search_time_is_over() {
    let fake = Fake::new(vec![done(0, GVFS_FIRST, ""), done(0, GVFS_LATER, "")]);
    let (result, listing, batches) = run(&fake, &FsPath::network(), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(batches[..2], [0, 4], "{SOURCE}: the first answer has no servers yet, the second brings four");
    assert!(batches[2..].iter().all(|&n| n == 4), "the same servers are not added twice: {batches:?}");
    assert_eq!(shown(&listing).len(), 4);
    assert_eq!(shown(&listing)[0], ("BRANO".to_owned(), "smb://brano-2.local:445/".to_owned()));
    assert_eq!(fake.calls(), DISCOVER_FOR.as_millis().div_ceil(ROUND.as_millis()) as usize);
    assert!(fake.asked.borrow().iter().all(|(uri, wait)| uri == NETWORK_ROOT && *wait <= Duration::from_secs(5)));
    assert!(fake.browsed.get().is_none(), "Avahi is not asked when gvfs answers");
}

#[test]
fn leaving_the_network_view_stops_the_search() {
    let fake = Fake::new(vec![done(0, GVFS_LATER, "")]);
    let (result, listing, batches) = run(&fake, &FsPath::network(), |_| false);
    assert_eq!(result, Ok(()));
    assert_eq!(fake.calls(), 1);
    assert_eq!(batches, [4]);
    assert_eq!(listing.len(), 4);
}

#[test]
fn the_network_order_uses_display_names_and_the_shared_sort_keys() {
    let fake = Fake::new(vec![done(0, GVFS_LATER, "")]);
    let mut keep = |_: &ListingBuilder<'_>| false;
    let mut out = ListingBuilder::new(CaseMode::Insensitive, &mut keep);
    read_dir_with(&fake, &FsPath::network(), &mut out, 0).unwrap();
    let mut order = Vec::new();
    flit_core::sort::sort(&out.listing, &out.keys, flit_core::sort::ViewSpec::default(), &mut order);
    let visible: Vec<_> = order.into_iter().map(|i| std::str::from_utf8(out.listing.label(i)).unwrap()).collect();
    assert_eq!(visible, ["BRANO", "nfs export", "Team DAV", "Файлы на сервере"]);
}

#[test]
fn a_hung_gvfs_uses_only_the_available_search_time() {
    let time_out = || Err(io::Error::from(io::ErrorKind::TimedOut));
    let fake = Fake::new(vec![time_out(), time_out(), time_out()]);
    let last_poll = DISCOVER_FOR - 2 * (PROBE_WAIT + TICK);
    fake.delays.borrow_mut().extend([PROBE_WAIT, PROBE_WAIT, last_poll]);
    let started = fake.now();
    let (result, listing, batches) = run(&fake, &FsPath::network(), |_| true);
    assert_eq!(result, Err(FsError::not_responding()));
    assert_eq!(batches, [0, 0, 0]);
    assert_eq!(fake.asked.borrow()[2].1, last_poll);
    assert!(listing.is_empty());
    assert_eq!(fake.now() - started, DISCOVER_FOR);
}

#[test]
fn a_cold_gvfs_can_publish_results_after_two_command_timeouts() {
    let time_out = || Err(io::Error::from(io::ErrorKind::TimedOut));
    let fake = Fake::new(vec![time_out(), time_out(), done(0, GVFS_LATER, "")]);
    fake.delays.borrow_mut().extend([PROBE_WAIT, PROBE_WAIT]);
    let (result, listing, batches) = run(&fake, &FsPath::network(), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(batches[..3], [0, 0, 4]);
    assert_eq!(listing.len(), 4);
}

#[test]
fn without_gvfs_network_avahi_finds_the_computers() {
    for gio in [Err(io::Error::from(io::ErrorKind::NotFound)), done(2, "", NOT_SUPPORTED)] {
        let fake = Fake::new(vec![gio]).avahi(AVAHI);
        let (result, listing, batches) = run(&fake, &FsPath::network(), |_| true);
        assert_eq!(result, Ok(()));
        assert_eq!(fake.calls(), 1, "gvfs is not asked again once it is known to be missing");
        assert_eq!(shown(&listing), [("BRANO".to_owned(), "smb://brano-2.local/".to_owned()), ("Flit проверка NFS".to_owned(), "nfs://brano-2.local/export".to_owned()), ("Team DAV".to_owned(), "dav://alice@brano-2.local:8080/dav".to_owned()), ("Файлы на сервере".to_owned(), "sftp://brano-2.local/".to_owned())]);
        assert_eq!(batches.into_iter().collect::<std::collections::BTreeSet<_>>(), [0, 1, 2, 3, 4].into_iter().collect());
        assert_eq!(fake.browsed.get(), Some(Duration::from_secs(5)));
    }
}

#[test]
fn without_gvfs_and_avahi_the_package_is_named_instead_of_waiting() {
    for gio in [Err(io::Error::from(io::ErrorKind::NotFound)), done(2, "", NOT_SUPPORTED)] {
        let fake = Fake::new(vec![gio]);
        let (result, listing, _) = run(&fake, &FsPath::network(), |_| true);
        assert_eq!(result, Err(FsError::no_network_browser(0)));
        assert_eq!(result.unwrap_err().network_package(), Some("gvfs-backends"));
        assert!(listing.is_empty());
        assert_eq!(fake.calls(), 1, "nothing is retried when nothing can answer");
    }
}

#[test]
fn a_hung_gvfs_without_avahi_reports_not_responding() {
    let fake = Fake::new((0..64).map(|_| Err(io::Error::from(io::ErrorKind::TimedOut))).collect());
    let (result, ..) = run(&fake, &FsPath::network(), |_| true);
    assert_eq!(result, Err(FsError::not_responding()));
}

#[test]
fn the_real_search_without_gio_and_avahi_browse_returns_at_once() {
    let started = Instant::now();
    let result = crate::temp::with_tools(&[], || {
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
        read_dir_with(&System, &FsPath::network(), &mut b, 0)
    });
    assert_eq!(result, Err(FsError::no_network_browser(0)));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn a_large_gio_listing_is_drained_before_the_deadline() {
    let mut cmd = std::process::Command::new("/usr/bin/head");
    cmd.args(["-c", "262144", "/dev/zero"]);
    let output = crate::places::run_within(cmd, PROBE_WAIT).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 262_144);
}

#[test]
fn a_server_lists_its_shares_and_mounts_the_server_first_when_needed() {
    let fake = Fake::new(vec![done(0, SHARES, "")]);
    let (result, listing, _) = run(&fake, &server("smb://brano-2.local:445/"), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(fake.asked.borrow()[0], ("smb://brano-2.local:445/".to_owned(), PROBE_WAIT));
    assert!(fake.mounted.borrow().is_empty());
    assert_eq!(shown(&listing)[0], ("private".to_owned(), "smb://brano-2.local/private".to_owned()));
    assert!(listing.entries.iter().all(|e| e.is_dir()));
    let fake = Fake::new(vec![done(2, "", NOT_MOUNTED), done(0, SHARES, "")]);
    let (result, listing, _) = run(&fake, &server("smb://brano.local/"), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(*fake.mounted.borrow(), ["smb://brano.local/"]);
    assert_eq!(listing.len(), 2);
}

#[test]
fn a_server_that_refuses_or_needs_a_password_shows_an_error() {
    let fake = Fake::new(vec![done(2, "", NOT_MOUNTED), done(2, "", NOT_MOUNTED)]);
    let (result, ..) = run(&fake, &server("smb://nas/"), |_| true);
    assert_eq!(result, Err(FsError { kind: FsErrorKind::AccessDenied, code: 0 }));
    let fake = Fake::new(vec![done(2, "", REFUSED)]);
    let (result, ..) = run(&fake, &server("smb://127.0.0.1/"), |_| true);
    assert_eq!(result, Err(FsError { kind: FsErrorKind::Unavailable, code: 0 }));
    assert!(fake.mounted.borrow().is_empty(), "a refused connection is not retried as a mount");
    let fake = Fake::new(vec![Err(io::Error::from(io::ErrorKind::NotFound))]);
    let (result, ..) = run(&fake, &server("smb://nas/"), |_| true);
    assert_eq!(result, Err(FsError::no_network_browser(0)));
}

#[test]
fn addresses_that_are_not_smb_servers_are_not_listed_as_folders() {
    let fake = Fake::new(Vec::new());
    for path in [server("sftp://brano-2.local:22/"), server("smb://nas/docs")] {
        let (result, ..) = run(&fake, &path, |_| true);
        assert_eq!(result, Err(FsError { kind: FsErrorKind::NotADirectory, code: 0 }));
    }
    assert_eq!(fake.calls(), 0);
}

#[test]
fn the_network_view_is_listed_off_the_shared_folder_threads_and_shown_in_the_sidebar() {
    let fs = crate::fs::LinuxGuiFs;
    assert!(flit_platform::Fs::remote(&fs, &FsPath::network()));
    assert!(flit_platform::Fs::remote(&fs, &server("smb://nas/")));
    assert!(!flit_platform::Fs::remote(&fs, &FsPath::parse_exact(b"/").unwrap()));
    let d = crate::places::network_drive();
    assert!(d.path.is_network() && d.path.is_root());
    assert_eq!(d.title(), flit_core::i18n::S::PathNetwork.t());
    assert!(!d.unmountable() && !d.mountable() && d.remote.is_none());
}

#[test]
#[ignore = "needs a private gvfsd session with published services: FLIT_NET_LIVE_BUS, FLIT_NET_LIVE_RUNTIME, FLIT_NET_LIVE_NAME and FLIT_NET_LIVE_SERVER"]
fn live_network_search_and_share_listing() {
    for (var, from) in [("DBUS_SESSION_BUS_ADDRESS", "FLIT_NET_LIVE_BUS"), ("XDG_RUNTIME_DIR", "FLIT_NET_LIVE_RUNTIME")] {
        std::env::set_var(var, std::env::var(from).expect(from));
    }
    let name = std::env::var("FLIT_NET_LIVE_NAME").expect("FLIT_NET_LIVE_NAME");
    let server_uri = std::env::var("FLIT_NET_LIVE_SERVER").expect("FLIT_NET_LIVE_SERVER");
    let started = Instant::now();
    let mut batches = Vec::new();
    let mut go = |b: &ListingBuilder<'_>| {
        batches.push((started.elapsed(), b.listing.len()));
        true
    };
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    let result = read_dir_with(&System, &FsPath::network(), &mut b, 0);
    let listing = std::mem::take(&mut b.listing);
    drop(b);
    println!("live-network result={result:?} batches={batches:?}");
    println!("live-network items={:?}", shown(&listing));
    assert_eq!(result, Ok(()));
    assert!(shown(&listing).iter().any(|(label, _)| *label == name), "{name} not found");
    assert!(started.elapsed() < DISCOVER_FOR + PROBE_WAIT + Duration::from_secs(2));
    let mounted = System.mount(&server_uri, PROBE_WAIT).unwrap();
    assert!(mounted.status.success(), "{}", String::from_utf8_lossy(&mounted.stderr));
    let (events, _) = crossbeam_channel::unbounded();
    let prompt = flit_platform::MountPrompt::new(1, events, std::sync::Arc::new(|| {}));
    assert!(crate::gvfs::browse(&server_uri, &prompt).unwrap());
    let raw = System.list(&server_uri, PROBE_WAIT).unwrap();
    println!("live-raw status={:?} out={:?} err={:?}", raw.status, String::from_utf8_lossy(&raw.stdout), String::from_utf8_lossy(&raw.stderr));
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    let result = read_dir_with(&System, &server(&server_uri), &mut b, 0);
    println!("live-shares result={result:?} items={:?}", shown(&b.listing));
    assert_eq!(result, Ok(()));
    assert!(!b.listing.is_empty());
    drop(b);
    if let Ok(locked) = std::env::var("FLIT_NET_LIVE_LOCKED") {
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
        let result = read_dir_with(&System, &server(&locked), &mut b, 0);
        println!("live-locked result={result:?}");
        assert_eq!(result, Err(FsError { kind: FsErrorKind::AccessDenied, code: 0 }));
    }
}
