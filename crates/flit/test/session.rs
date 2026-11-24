use super::*;
use flit_core::Location;

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-session-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Temp(d)
}

fn window(paths: &[&str], cur: usize) -> WindowSession {
    WindowSession { tabs: paths.iter().map(|p| Location::parse(p).unwrap()).collect(), cur, ..Default::default() }
}

#[test]
fn opening_from_the_tray_uses_the_saved_windows_or_chosen_startup_mode() {
    use flit_core::ui::background::{idle, Idle};

    let at = Placement { x: 7, y: 14, width: 920, height: 620, maximized: true };
    let last = (window(&["/tmp", "/usr"], 1), Some(at));
    let other = (window(&["/etc"], 0), None);
    let previous = Session::new(vec![last.clone(), other.clone()], vec!["history".into()]);
    assert_eq!(idle(true, Some(true), false, false), Idle::Wait);
    assert_eq!(idle(true, Some(true), false, true), Idle::Open);
    assert_eq!(startup_windows(&Startup::Continue, Some(&previous), true, false), [last, other]);
    assert_eq!(startup_windows(&Startup::NewTab, Some(&previous), true, false)[0].0.tabs, [Location::home_page()]);
    let folders = vec![Location::parse("/home").unwrap(), Location::parse("/var").unwrap()];
    assert_eq!(startup_windows(&Startup::Folders(folders.clone()), Some(&previous), true, false)[0].0.tabs, folders);
    assert!(startup_windows(&Startup::Continue, Some(&previous), false, false).is_empty());
}

#[test]
fn quitting_in_the_tray_keeps_the_closed_session_and_quitting_before_opening_keeps_the_previous_session() {
    let d = temp("tray-quit");
    let files = Files { session: d.0.join("session.bin"), marker: d.0.join("running") };
    let saved = Session::new(vec![(window(&["/tmp", "/usr"], 1), None)], vec!["search".into()]);
    let writer = Writer::start(files.session.clone());
    let closed = saved.windows().next().unwrap();
    let snapshot = Session::new(vec![closed], saved.typed.clone());
    files.mark();
    writer.send(Session::new(vec![(window(&["/tmp"], 0), None)], Vec::new()));
    drop(writer);
    files.finish(Some(&snapshot));
    assert_eq!(load(&files.session), Some(saved.clone()));
    assert!(!files.crashed());

    files.finish(None);
    assert_eq!(load(&files.session), Some(saved));
}

#[test]
fn linux_last_session_resumes_with_a_settled_crash_marker() {
    assert!(restore_crash(&Startup::Continue, true, true));
    assert!(!restore_crash(&Startup::Continue, true, false), "a run that died early keeps the recovery prompt");
    assert!(!restore_crash(&Startup::Continue, false, true), "Windows keeps its recovery prompt");
    assert!(!restore_crash(&Startup::NewTab, true, true));
    assert!(!restore_crash(&Startup::Folders(vec![Location::home_page()]), true, true));
}

#[test]
fn the_window_for_the_final_requested_location_is_mapped_last() {
    use flit_core::open::Route;
    let w = |path: &str| (window(&[path], 0), None);
    let mut saved = std::collections::VecDeque::from([w("/etc"), w("/usr"), w("/tmp")]);
    let routes: Vec<_> = ["/tmp", "/etc", "/var/log"].into_iter().map(|p| Route { folder: Location::parse(p).unwrap(), select: None }).collect();
    open_requested_window_last(&mut saved, &routes);
    assert_eq!(saved, [w("/usr"), w("/tmp"), w("/etc")]);
}

#[test]
fn only_a_run_that_saved_its_session_a_minute_after_start_counts_as_settled() {
    let d = temp("settled");
    std::fs::create_dir_all(&d.0).unwrap();
    let files = Files { session: d.0.join("session.bin"), marker: d.0.join("running") };
    assert!(!files.settled_before_exit(), "no marker and no session");
    files.mark();
    std::fs::write(&files.session, b"x").unwrap();
    let started = files.marked_at().unwrap();
    let touch = |at: std::time::SystemTime| std::fs::File::options().write(true).open(&files.session).unwrap().set_modified(at).unwrap();
    touch(started + std::time::Duration::from_secs(5));
    assert!(!files.settled_before_exit(), "a crash seconds after start must not restore the same session again");
    touch(started + std::time::Duration::from_secs(120));
    assert!(files.settled_before_exit());
    touch(started - std::time::Duration::from_secs(600));
    assert!(!files.settled_before_exit(), "a session older than the run says nothing about it");
}

#[test]
fn sessions_round_trip_and_skip_empty_windows() {
    let at = Placement { x: 10, y: 20, width: 800, height: 600, maximized: true };
    let s = Session::new(vec![(window(&[r"C:\a", "wsl://Ubuntu/home"], 1), Some(at)), (window(&[], 0), None), (window(&[r"D:\"], 0), None)], vec![r"C:\typed".into()]);
    let back = Session::decode(&s.encode().unwrap()).unwrap();
    assert_eq!(back, s);
    let windows: Vec<_> = back.windows().collect();
    assert_eq!(windows, [(window(&[r"C:\a", "wsl://Ubuntu/home"], 1), Some(at)), (window(&[r"D:\"], 0), None)]);
    assert_eq!(back.typed, [r"C:\typed"]);
    let mut old = s.clone();
    old.version = VERSION + 1;
    assert_eq!(Session::decode(&old.encode().unwrap()), None);
    assert_eq!(Session::decode(b"junk"), None);
}

#[test]
fn session_files_from_a_newer_flit_are_set_aside_and_broken_ones_go_to_bad() {
    let d = temp("schema");
    let f = d.0.join("session.bin");
    let s = Session::new(vec![(window(&[r"C:\a"], 0), None)], vec!["typed".into()]);
    write(&f, &s);
    assert_eq!(load(&f), Some(s.clone()));
    let mut newer = s.clone();
    newer.version = VERSION + 1;
    write(&f, &newer);
    assert_eq!(load(&f), None);
    assert_eq!(std::fs::read(crate::schema::aside(&f, crate::schema::NEWER)).unwrap(), newer.encode().unwrap());
    std::fs::write(&f, [1u8, 0xFF, 0xFF]).unwrap();
    assert_eq!(load(&f), None);
    assert!(crate::schema::aside(&f, crate::schema::BAD).exists());
}

#[test]
fn the_writer_keeps_the_latest_session_and_the_marker_tells_a_crash() {
    let d = temp("writer");
    let files = Files { session: d.0.join("session.bin"), marker: d.0.join("running") };
    assert!(!files.crashed());
    files.mark();
    assert!(files.crashed());
    let mut w = Writer::start(files.session.clone());
    for k in 0..50 {
        w.send(Session::new(vec![(window(&[r"C:\x"], 0), None)], vec![format!("{k}")]));
    }
    w.finish();
    assert_eq!(load(&files.session).map(|s| s.typed), Some(vec!["49".to_owned()]));
    files.unmark();
    assert!(!files.crashed());
    assert!(load(&d.0.join("missing.bin")).is_none());
}

#[test]
fn linux_locations_are_durable_before_teardown_and_writer_drop_drains_pending_snapshots() {
    let d = temp("linux-teardown");
    let f = d.0.join("session.bin");
    let first = Session::new(vec![(window(&["/home/alice", "/tmp", "/"], 1), None)], vec!["/var/log".into()]);
    let writer = Writer::start(f.clone());
    writer.flush(first.clone());
    assert_eq!(load(&f), Some(first), "flush persists the snapshot before the window is destroyed");
    let last = Session::new(vec![(window(&["/home/alice/Documents", "/tmp"], 0), None)], Vec::new());
    writer.send(last.clone());
    drop(writer);
    assert_eq!(load(&f), Some(last), "dropping a writer joins its final write");
    assert!(!f.with_extension(format!("{}.tmp", std::process::id())).exists());
}

#[test]
fn panels_ride_after_the_session_and_files_with_per_tab_panes_still_load() {
    use flit_core::ui::{PanelsSession, Shape};
    let mut w = window(&[r"C:\a", r"C:\b"], 1);
    w.trails = vec![Location::parse(r"C:\a\deep\er").unwrap(), Location::parse(r"C:\b").unwrap()];
    let leaf = Shape::Leaf { tabs: 1, cur: 0 };
    w.panels = Some(PanelsSession { root: Shape::Group { down: true, kids: vec![(leaf.clone(), 0.3), (leaf, 0.7)] }, active: 1 });
    let s = Session::new(vec![(w.clone(), None), (window(&[r"E:\"], 0), None)], Vec::new());
    let bytes = s.encode().unwrap();
    let back = Session::decode(&bytes).unwrap();
    assert_eq!(back.windows().next().unwrap().0, w);
    let older = postcard::to_stdvec(&s).unwrap();
    assert!(bytes.starts_with(&older) && bytes.len() > older.len());
    let mut previous_encoding = older.clone();
    previous_encoding.extend(postcard::to_stdvec(&[Vec::<Option<SplitSession>>::new(), Vec::new()][..]).unwrap());
    previous_encoding.extend(postcard::to_stdvec(&[w.trails.clone(), Vec::new()][..]).unwrap());
    previous_encoding.extend(postcard::to_stdvec(&[w.panels.clone(), None][..]).unwrap());
    assert_eq!(bytes, previous_encoding);
    let old = Session::decode(&older).unwrap();
    assert_eq!(old.windows().next().unwrap().0, window(&[r"C:\a", r"C:\b"], 1));
    assert_eq!(postcard::from_bytes::<Session>(&bytes).unwrap().windows().next().unwrap().0.tabs, w.tabs, "older builds read the tabs and skip the panels");
    let pane = SplitSession { other: Location::parse(r"D:\x").unwrap(), dir: SplitDir::Horizontal, ratio: 0.3, second: true };
    let mut legacy = older.clone();
    legacy.extend(postcard::to_stdvec(&vec![vec![None, Some(pane)], vec![None]]).unwrap());
    legacy.extend(postcard::to_stdvec(&vec![w.trails.clone(), vec![Location::parse(r"E:\").unwrap()]]).unwrap());
    let loaded = Session::decode(&legacy).unwrap().windows().next().unwrap().0;
    assert_eq!(loaded, WindowSession { panels: None, ..w.clone() }, "a file with a second pane per tab loads its tabs and trails in one panel");
}

#[test]
fn session_serializes_network_tabs_and_trails_compatibly_with_the_running_platform() {
    let root = Location::from_local(flit_core::FsPath::network());
    let server = Location::from_local(flit_core::FsPath::network_server(b"NAS").unwrap());
    let local = Location::parse("/tmp").unwrap();
    let home = Location::from_local(<crate::P as flit_platform::ShellPlatform>::home());
    let network = |path: &Location| if cfg!(target_os = "linux") { home.clone() } else { path.clone() };
    let w = WindowSession { tabs: vec![root.clone(), server.clone(), local.clone()], cur: 1, trails: vec![server.clone(), root.clone(), local.clone()], panels: None };
    let s = Session::new(vec![(w, None)], Vec::new());
    let restored = Session::decode(&s.encode().unwrap()).unwrap();
    let (restored, _) = restored.windows().next().unwrap();
    assert_eq!(restored.tabs, [network(&root), network(&server), local.clone()]);
    assert_eq!(restored.trails, [network(&server), network(&root), local]);
    assert_eq!(restored.cur, 1);
}
