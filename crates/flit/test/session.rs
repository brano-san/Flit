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
fn panes_ride_after_the_session_so_older_files_load_with_one_pane() {
    use flit_core::ui::{SplitDir, SplitSession};
    let mut w = window(&[r"C:\a", r"C:\b"], 1);
    w.splits = vec![None, Some(SplitSession { other: Location::parse(r"D:\x").unwrap(), dir: SplitDir::Horizontal, ratio: 0.3, second: true })];
    let s = Session::new(vec![(w.clone(), None), (window(&[r"E:\"], 0), None)], Vec::new());
    let bytes = s.encode().unwrap();
    let back = Session::decode(&bytes).unwrap();
    assert_eq!(back.windows().next().unwrap().0, w);
    let older = postcard::to_stdvec(&s).unwrap();
    assert!(bytes.starts_with(&older) && bytes.len() > older.len());
    let old = Session::decode(&older).unwrap();
    assert_eq!(old.windows().next().unwrap().0, window(&[r"C:\a", r"C:\b"], 1));
    assert_eq!(postcard::from_bytes::<Session>(&bytes).unwrap().windows().next().unwrap().0.tabs, w.tabs, "older builds read the tabs and skip the panes");
}
