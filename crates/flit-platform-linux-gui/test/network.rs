use super::*;
use flit_core::sort::CaseMode;
use flit_core::Listing;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

#[test]
fn a_missing_discovery_component_is_reported_with_its_distribution_package() {
    use flit_core::provider::NetworkWarning;
    assert_eq!(discovery_warning(Some(false), false, "ID=debian"), Some(NetworkWarning::LinuxMissing {
        avahi: Some("avahi-daemon"), gvfs: Some("gvfs-backends"),
    }));
    assert_eq!(discovery_warning(Some(false), true, "ID=fedora"), Some(NetworkWarning::LinuxMissing {
        avahi: Some("avahi"), gvfs: None,
    }));
    assert_eq!(discovery_warning(Some(true), true, "ID=arch"), None);
    assert_eq!(discovery_warning(None, true, "ID=opensuse"), None);
}

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

const WORKGROUPS: &str = "WORKGROUP\t0\t(shortcut)\tstandard::display-name=WORKGROUP standard::target-uri=smb://WORKGROUP/\nLAB\t0\t(shortcut)\tstandard::display-name=Lab\n";
const COMPUTERS: &str = "BRANO-2\t0\t(shortcut)\tstandard::display-name=BRANO standard::target-uri=smb://BRANO-2/\nFLITWIN\t0\t(shortcut)\tstandard::display-name=FLITWIN standard::target-uri=smb://FLITWIN/\n";

struct Fake {
    lists: RefCell<VecDeque<io::Result<Output>>>,
    asked: RefCell<Vec<(String, Duration)>>,
    mounted: RefCell<Vec<String>>,
}

impl Fake {
    fn new(lists: Vec<io::Result<Output>>) -> Self {
        Self { lists: RefCell::new(lists.into()), asked: RefCell::default(), mounted: RefCell::default() }
    }
}

impl Probe for Fake {
    fn list(&self, uri: &str, wait: Duration) -> io::Result<Output> {
        self.asked.borrow_mut().push((uri.to_owned(), wait));
        self.lists.borrow_mut().pop_front().expect("unexpected list")
    }

    fn mount(&self, uri: &str, _wait: Duration) -> io::Result<Output> {
        self.mounted.borrow_mut().push(uri.to_owned());
        done(0, "", "")
    }
}

struct Schedule {
    start: Instant,
    clock: Instant,
    asked: Vec<(Query, Duration, Duration)>,
    network: VecDeque<(Duration, Listed)>,
    avahi: Vec<(Duration, Reply)>,
    groups: Vec<(&'static str, Vec<Found>)>,
    pending: Vec<(Instant, Reply)>,
}

impl Schedule {
    fn new(network: Vec<(Duration, Listed)>) -> Self {
        let start = Instant::now();
        Self { start, clock: start, asked: Vec::new(), network: network.into(), avahi: Vec::new(), groups: Vec::new(), pending: Vec::new() }
    }

    fn quick(texts: &[&str]) -> Self {
        Self::new(texts.iter().map(|text| (Duration::ZERO, Listed::Text((*text).into()))).collect())
    }

    fn elapsed(&self) -> Duration {
        self.clock - self.start
    }
}

impl Discovery for Schedule {
    fn now(&self) -> Instant {
        self.clock
    }

    fn start(&mut self, query: Query, until: Instant) {
        let wait = until.duration_since(self.clock).min(PROBE_WAIT);
        self.asked.push((query.clone(), self.elapsed(), wait));
        match query {
            Query::Network => {
                let (delay, listed) = self.network.pop_front().unwrap_or((Duration::ZERO, Listed::Text(String::new())));
                let reply = if delay >= wait { Listed::Failed(io::ErrorKind::TimedOut.into()) } else { listed };
                self.pending.push((self.clock + delay.min(wait), Reply::Network(reply)));
            }
            Query::Avahi if self.avahi.is_empty() => self.pending.push((self.clock, Reply::AvahiDone(Err(io::ErrorKind::NotFound.into())))),
            Query::Avahi => self.pending.extend(std::mem::take(&mut self.avahi).into_iter().map(|(elapsed, reply)| (self.clock + elapsed, reply))),
            Query::Smb(uri) => {
                let (index, _) = self.groups.iter().enumerate().find(|(_, (u, _))| *u == uri).expect("unexpected SMB query");
                let (_, found) = self.groups.remove(index);
                let reply = if uri == SMB_ROOT { Reply::Groups(found) } else { Reply::Smb(found) };
                self.pending.push((self.clock + Duration::from_millis(100), reply));
            }
        }
    }

    fn receive(&mut self, wait: Duration) -> Option<Reply> {
        let next = self.pending.iter().enumerate().filter(|(_, (at, _))| *at <= self.clock + wait).min_by_key(|(_, (at, _))| *at).map(|(i, _)| i);
        if let Some(i) = next {
            let (at, reply) = self.pending.remove(i);
            self.clock = at;
            Some(reply)
        } else {
            self.clock += wait;
            None
        }
    }
}

fn run(enumerate: impl FnOnce(&mut ListingBuilder<'_>) -> Result<(), FsError>, mut keep: impl FnMut(usize) -> bool) -> (Result<(), FsError>, Listing, Vec<usize>) {
    let mut batches = Vec::new();
    let mut go = |b: &ListingBuilder<'_>| {
        batches.push(b.listing.len());
        keep(b.listing.len())
    };
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    let result = enumerate(&mut b);
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
            found("Windows Network", SMB_ROOT),
        ],
        "{SOURCE}"
    );
    assert_eq!(parse_network(GVFS_FIRST), [found("Windows Network", SMB_ROOT)]);
    assert_eq!(parse_network("smb-root\t0\t(shortcut)\tstandard::name=smb-root standard::display-name=Windows Network\n")[0].uri, SMB_ROOT);
    assert_eq!(parse_network(GVFS_WITH_ICON)[0].uri, SMB_ROOT);
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
fn fast_network_discovery_finishes_after_two_unchanged_rounds_and_publishes_immediately() {
    let sample = "box\t0\t(shortcut)\tstandard::display-name=Box standard::target-uri=sftp://box/\n";
    let mut p = Schedule::quick(&[sample, sample, sample]);
    p.avahi.push((Duration::from_secs(5), Reply::AvahiDone(Ok(()))));
    let (result, listing, batches) = run(|out| discover(&mut p, out), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(batches.first(), Some(&1));
    assert_eq!(listing.len(), 1);
    assert_eq!(p.elapsed(), Duration::from_secs(2));
    assert_eq!(p.asked.iter().filter(|(q, ..)| q == &Query::Network).map(|(_, time, _)| time.as_secs()).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(p.asked[1], (Query::Avahi, Duration::ZERO, PROBE_WAIT));
}

#[test]
fn new_addresses_restart_the_two_unchanged_rounds() {
    let one = "box\t0\t(shortcut)\tstandard::display-name=Box standard::target-uri=sftp://box/\n";
    let two = format!("{one}box2\t0\t(shortcut)\tstandard::display-name=Second standard::target-uri=ftp://box2/\n");
    let mut p = Schedule::quick(&[one, one, &two, &two, &two]);
    let (result, listing, _) = run(|out| discover(&mut p, out), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(p.elapsed(), Duration::from_secs(4));
    assert_eq!(shown(&listing).len(), 2);
}

#[test]
fn hung_commands_cannot_extend_discovery_past_six_seconds() {
    let slow = (Duration::from_secs(30), Listed::Text(String::new()));
    let mut p = Schedule::new(vec![slow, (Duration::from_secs(30), Listed::Text(String::new()))]);
    p.avahi.push((Duration::from_secs(30), Reply::AvahiDone(Ok(()))));
    let (result, listing, _) = run(|out| discover(&mut p, out), |_| true);
    assert_eq!(result, Ok(()));
    assert!(listing.is_empty());
    assert_eq!(p.elapsed(), Duration::from_secs(6));
    assert_eq!(p.asked, [
        (Query::Network, Duration::ZERO, Duration::from_secs(5)),
        (Query::Avahi, Duration::ZERO, Duration::from_secs(5)),
        (Query::Network, Duration::from_millis(5250), Duration::from_millis(750)),
    ]);
}

#[test]
fn smb_groups_and_computers_follow_gio_attributes() {
    assert_eq!(parse_smb_browse(WORKGROUPS).collect::<Vec<_>>(), [found("WORKGROUP", "smb://WORKGROUP/"), found("Lab", "smb://LAB/")]);
    assert_eq!(parse_smb_browse(COMPUTERS).collect::<Vec<_>>(), [found("BRANO", "smb://BRANO-2/"), found("FLITWIN", "smb://FLITWIN/")]);
    assert!(parse_smb_browse("ignored\t0\t(mountable)\tstandard::target-uri=smb://HOST/share\n").next().is_none());
}

#[test]
fn smb_browse_opens_workgroups_and_only_adds_unique_computers() {
    let mut p = Schedule::quick(&[GVFS_LATER, GVFS_FIRST, GVFS_FIRST]);
    p.groups = vec![
        (SMB_ROOT, parse_smb_browse(WORKGROUPS).collect()),
        ("smb://WORKGROUP/", parse_smb_browse(COMPUTERS).collect()),
        ("smb://LAB/", vec![found("Duplicate", "smb://flitwin/")]),
    ];
    let (result, listing, batches) = run(|out| discover(&mut p, out), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(batches[0], 4);
    assert_eq!(listing.len(), 5);
    let listed = shown(&listing);
    assert!(listed.contains(&("FLITWIN".into(), "smb://FLITWIN/".into())));
    assert!(listed.iter().all(|(_, uri)| uri != SMB_ROOT && uri != "smb://WORKGROUP/"));
    assert!(p.asked.iter().any(|(q, ..)| q == &Query::Smb("smb://LAB/".into())));
    assert_eq!(p.elapsed(), Duration::from_secs(3));
}

#[test]
fn an_unavailable_windows_network_container_leaves_the_view_empty() {
    let mut p = Schedule::quick(&[GVFS_FIRST, GVFS_FIRST, GVFS_FIRST]);
    p.groups = vec![(SMB_ROOT, Vec::new())];
    let (result, listing, _) = run(|out| discover(&mut p, out), |_| true);
    assert_eq!(result, Ok(()));
    assert!(listing.is_empty());
    assert_eq!(p.elapsed(), Duration::from_secs(2));
    assert_eq!(p.asked.iter().filter(|(query, ..)| query == &Query::Smb(SMB_ROOT.into())).count(), 1);
}

#[test]
fn leaving_the_network_view_stops_the_search() {
    let mut p = Schedule::quick(&[GVFS_LATER]);
    p.groups = vec![(SMB_ROOT, Vec::new())];
    let (result, listing, batches) = run(|out| discover(&mut p, out), |_| false);
    assert_eq!(result, Ok(()));
    assert_eq!(p.asked.iter().filter(|(query, ..)| query == &Query::Network).count(), 1);
    assert_eq!(batches, [4]);
    assert_eq!(listing.len(), 4);
}

#[test]
fn the_network_order_uses_display_names_and_the_shared_sort_keys() {
    let mut p = Schedule::quick(&[GVFS_LATER]);
    p.groups = vec![(SMB_ROOT, Vec::new())];
    let mut keep = |_: &ListingBuilder<'_>| false;
    let mut out = ListingBuilder::new(CaseMode::Insensitive, &mut keep);
    discover(&mut p, &mut out).unwrap();
    let mut order = Vec::new();
    flit_core::sort::sort(&out.listing, &out.keys, flit_core::sort::ViewSpec::default(), &mut order);
    let visible: Vec<_> = order.into_iter().map(|i| std::str::from_utf8(out.listing.label(i)).unwrap()).collect();
    assert_eq!(visible, ["BRANO", "nfs export", "Team DAV", "Файлы на сервере"]);
}

#[test]
fn avahi_has_results_ready_when_gio_reports_it_cannot_browse() {
    let mut p = Schedule::new(vec![(Duration::from_secs(4), Listed::Unsupported)]);
    p.avahi = vec![
        (Duration::from_millis(500), Reply::AvahiItem(found("Box", "smb://box.local/"))),
        (Duration::from_secs(1), Reply::AvahiDone(Ok(()))),
    ];
    let (result, listing, _) = run(|out| discover(&mut p, out), |n| n == 0);
    assert_eq!(result, Ok(()));
    assert_eq!(p.elapsed(), Duration::from_secs(4));
    assert_eq!(p.asked[1], (Query::Avahi, Duration::ZERO, PROBE_WAIT));
    assert_eq!(listing.len(), 1);
}

#[test]
fn a_missing_avahi_program_is_distinguished_from_a_stopped_daemon() {
    for (avahi, expected) in [(io::ErrorKind::NotFound, Err(FsError::no_network_browser())), (io::ErrorKind::Other, Ok(()))] {
        let mut p = Schedule::new(vec![(Duration::ZERO, Listed::Unsupported)]);
        p.avahi.push((Duration::ZERO, Reply::AvahiDone(Err(io::Error::new(avahi, "Daemon not running")))));
        let (result, listing, _) = run(|out| discover(&mut p, out), |_| true);
        assert_eq!(result, expected);
        assert!(listing.is_empty());
        assert_eq!(p.elapsed(), Duration::ZERO);
    }
}

#[test]
fn the_real_search_without_gio_and_avahi_browse_returns_at_once() {
    let started = Instant::now();
    let result = crate::temp::with_tools(&[], || {
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
        read_dir(&FsPath::network(), &mut b)
    });
    assert_eq!(result, Err(FsError::no_network_browser()));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn advertised_labels_strip_control_characters_and_are_capped_by_characters() {
    let label = format!("a\\027\\009\u{202a}\u{202e}\u{2066}\u{2069}\u{200e}\u{200f}{}", "я".repeat(300));
    let parsed = parse_avahi(&format!("=;eth0;IPv4;{label};_smb._tcp;local;box.local;10.0.0.2;445;")).unwrap();
    assert_eq!(parsed.name, format!("a{}", "я".repeat(254)));
    let sample = "x\t0\t(shortcut)\tstandard::display-name=a\u{202e}b standard::target-uri=sftp://box/\n";
    assert_eq!(parse_network(sample)[0].name, "ab");
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
fn listing_server_shares_is_read_only_even_when_the_server_is_not_mounted() {
    let uri = "smb://brano-2.local:445/";
    let fake = Fake::new(vec![done(0, SHARES, ""), done(2, "", NOT_MOUNTED)]);
    let (result, listing, _) = run(|out| read_server(&fake, &server(uri), out), |_| true);
    assert_eq!(result, Ok(()));
    assert_eq!(fake.asked.borrow()[0], (uri.to_owned(), PROBE_WAIT));
    assert_eq!(shown(&listing)[0], ("private".to_owned(), "smb://brano-2.local/private".to_owned()));
    assert!(listing.entries.iter().all(|e| e.is_dir()));
    let (result, listing, _) = run(|out| read_server(&fake, &server(uri), out), |_| true);
    assert_eq!(result, Err(FsError { kind: FsErrorKind::AccessDenied, code: 0 }));
    assert!(listing.is_empty());
    assert!(fake.mounted.borrow().is_empty());
    assert!(mount_anonymous(&fake, uri));
    assert_eq!(*fake.mounted.borrow(), [uri]);
}

#[test]
fn refusal_and_missing_gio_stay_listing_errors_without_mounting() {
    for error in [done(2, "", REFUSED), Err(io::ErrorKind::NotFound.into())] {
        let fake = Fake::new(vec![error]);
        let (result, ..) = run(|out| read_server(&fake, &server("smb://127.0.0.1/"), out), |_| true);
        assert_eq!(result, Err(FsError { kind: FsErrorKind::Unavailable, code: 0 }));
        assert!(fake.mounted.borrow().is_empty());
    }
}

#[test]
fn addresses_that_are_not_smb_servers_are_not_listed_as_folders() {
    let fake = Fake::new(Vec::new());
    for path in [server("sftp://brano-2.local:22/"), server("smb://nas/docs")] {
        let (result, ..) = run(|out| read_server(&fake, &path, out), |_| true);
        assert_eq!(result, Err(FsError { kind: FsErrorKind::NotADirectory, code: 0 }));
    }
    assert!(fake.asked.borrow().is_empty());
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
    let result = read_dir(&FsPath::network(), &mut b);
    let listing = std::mem::take(&mut b.listing);
    drop(b);
    println!("live-network result={result:?} batches={batches:?}");
    println!("live-network items={:?}", shown(&listing));
    assert_eq!(result, Ok(()));
    assert!(shown(&listing).iter().any(|(label, _)| *label == name), "{name} not found");
    assert!(started.elapsed() < DISCOVER_FOR + Duration::from_secs(1));
    use flit_platform::Shell;
    let (events, _) = crossbeam_channel::unbounded();
    let prompt = flit_platform::MountPrompt::new(1, events, std::sync::Arc::new(|| {}));
    let mounted = crate::shell::LinuxShell.connect_asking(&server_uri, &prompt).unwrap();
    assert_eq!(mounted, Some(server(&server_uri)));
    let raw = System.list(&server_uri, PROBE_WAIT).unwrap();
    println!("live-raw status={:?} out={:?} err={:?}", raw.status, String::from_utf8_lossy(&raw.stdout), String::from_utf8_lossy(&raw.stderr));
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    let result = read_dir(&server(&server_uri), &mut b);
    println!("live-shares result={result:?} items={:?}", shown(&b.listing));
    assert_eq!(result, Ok(()));
    assert!(!b.listing.is_empty());
    drop(b);
    if let Ok(locked) = std::env::var("FLIT_NET_LIVE_LOCKED") {
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
        let result = read_dir(&server(&locked), &mut b);
        println!("live-locked result={result:?}");
        assert_eq!(result, Err(FsError { kind: FsErrorKind::AccessDenied, code: 0 }));
    }
}
