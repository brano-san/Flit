use super::*;
use crate::temp::Temp;
use flit_core::provider::{FileOp, Resolve};
use flit_platform::FileOps;
use std::cell::{Cell, RefCell};
use x11rb::connection::Connection;
use x11rb::wrapper::ConnectionExt as _;

const ATOM: Atom = 501;

struct Fake {
    owner: Option<Window>,
    name: Option<Vec<u8>>,
    asked: Cell<u32>,
    written: RefCell<Vec<(Window, Vec<u8>)>>,
}

impl Fake {
    fn new(owner: Option<Window>, name: Option<&[u8]>) -> Self {
        Self { owner, name: name.map(<[u8]>::to_vec), asked: Cell::new(0), written: RefCell::new(Vec::new()) }
    }
}

impl Link for Fake {
    fn direct_save(&self) -> Atom {
        ATOM
    }

    fn owner(&self) -> Option<Window> {
        self.asked.set(self.asked.get() + 1);
        self.owner
    }

    fn read_name(&self, _: Window) -> Option<Vec<u8>> {
        self.asked.set(self.asked.get() + 1);
        self.name.clone()
    }

    fn write_uri(&self, window: Window, uri: &[u8]) -> bool {
        self.written.borrow_mut().push((window, uri.to_vec()));
        true
    }
}

fn fs(path: &Path) -> FsPath {
    FsPath::parse_exact(path.as_os_str().as_encoded_bytes()).unwrap()
}

fn offer(name: &str) -> Offer {
    Offer { window: 77, name: name.as_bytes().to_vec(), atom: ATOM }
}

fn staged_path(uri: &[u8]) -> PathBuf {
    let rest = uri.strip_prefix(b"file://").expect("a local file URI");
    PathBuf::from(String::from_utf8(flit_platform_linux::percent_decode(rest)).unwrap())
}

fn paste(dest: &Path, data: &Arc<DroppedFile>, job: &Job) -> OpOutcome {
    crate::shell::LinuxShell.file_op(&FileOp::PasteDropped { dest: fs(dest), moving: false, data: Some(data.clone()) }, job)
}

#[test]
fn the_source_answers_saved_failed_and_error_with_one_letter() {
    assert_eq!(reply(b"S"), Reply::Saved);
    assert_eq!(reply(b"F"), Reply::Declined);
    assert_eq!(reply(b"E"), Reply::Failed);
    assert_eq!(reply(b""), Reply::Failed, "an empty answer is not a success");
    assert_eq!(reply(b"x"), Reply::Failed);
}

#[test]
fn the_name_offered_by_the_source_becomes_one_safe_file_name() {
    assert_eq!(suggested_name(b"photo 1.jpg").as_deref(), Some(&b"photo 1.jpg"[..]));
    assert_eq!(suggested_name(b"../../etc/passwd").as_deref(), Some(&b"passwd"[..]), "only the last part of a path is kept");
    assert_eq!(suggested_name(b"dir/").as_deref(), Some(&b"dir"[..]));
    assert_eq!(suggested_name(b" re\nport\t.txt ").as_deref(), Some(&b"report.txt"[..]), "control characters never reach the name");
    assert_eq!(suggested_name("отчёт.pdf".as_bytes()).as_deref(), Some("отчёт.pdf".as_bytes()));
    for bad in [&b""[..], b"/", b"..", b".", b"  ", b"\x01"] {
        assert_eq!(suggested_name(bad), None, "{bad:?}");
    }
    let long = "я".repeat(200);
    let cut = suggested_name(long.as_bytes()).unwrap();
    assert!(cut.len() <= MAX_NAME && std::str::from_utf8(&cut).is_ok(), "a long name is cut on a character boundary");
}

#[test]
fn a_drop_that_does_not_offer_direct_save_never_asks_the_x_server() {
    let link = Fake::new(Some(77), Some(b"a.txt"));
    assert_eq!(probe(&link, &[]), None);
    assert_eq!(probe(&link, &[ATOM + 1, ATOM + 2]), None);
    assert_eq!(link.asked.get(), 0, "ordinary drops of files, links, text and pictures stay on their old path");
}

#[test]
fn a_drop_offering_direct_save_names_the_source_window_and_the_suggested_file() {
    let link = Fake::new(Some(77), Some(b"archive/inner file.txt"));
    assert_eq!(probe(&link, &[ATOM + 1, ATOM]), Some(Offer { window: 77, name: b"inner file.txt".to_vec(), atom: ATOM }));
    assert_eq!(probe(&Fake::new(None, Some(b"a.txt")), &[ATOM]), None, "no source window");
    assert_eq!(probe(&Fake::new(Some(x11rb::NONE), Some(b"a.txt")), &[ATOM]), None, "the selection has no owner");
    assert_eq!(probe(&Fake::new(Some(77), None), &[ATOM]), None, "the source gave no name");
    assert_eq!(probe(&Fake::new(Some(77), Some(b"..")), &[ATOM]), None);
}

#[test]
fn the_source_is_told_to_save_into_a_private_folder_by_a_file_uri() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new("xds-uri");
    let pending = prepare(&offer("a b%.txt"), None, t.path()).unwrap();
    let staged = staged_path(&pending.uri);
    assert_eq!(staged.file_name().unwrap(), "a b%.txt");
    assert!(String::from_utf8(pending.uri.clone()).unwrap().ends_with("/a%20b%25.txt"), "{:?}", String::from_utf8_lossy(&pending.uri));
    let dir = staged.parent().unwrap();
    assert_eq!(dir.parent().unwrap(), t.path());
    assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!((pending.window, pending.data.name.as_slice(), pending.data.bytes.len()), (77, &b"a b%.txt"[..], 0));
    forget(&pending.data);
    assert!(!dir.exists(), "a drop Flit did not take leaves nothing behind");
}

#[test]
fn a_file_saved_by_the_source_lands_in_the_folder_and_is_reported_for_selection() {
    let t = Temp::new("xds-saved");
    let dest = t.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let pending = prepare(&offer("from archive.txt"), None, t.path()).unwrap();
    let staged = staged_path(&pending.uri);
    std::fs::write(&staged, b"extracted").unwrap();
    pending.sender.send(Reply::Saved).unwrap();
    let out = paste(&dest, &pending.data, &Job::detached(Resolve::Skip));
    assert!(out.error.is_none(), "{:?}", out.error);
    let target = fs(&dest.join("from archive.txt"));
    assert_eq!(out.items, [(target.clone(), target)]);
    assert_eq!(std::fs::read(dest.join("from archive.txt")).unwrap(), b"extracted");
    assert!(!staged.parent().unwrap().exists(), "the private folder is removed");
}

#[test]
fn a_saved_file_with_a_taken_name_follows_the_usual_conflict_choice() {
    let t = Temp::new("xds-conflict");
    let dest = t.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("same.txt"), b"old").unwrap();
    let pending = prepare(&offer("same.txt"), None, t.path()).unwrap();
    std::fs::write(staged_path(&pending.uri), b"new").unwrap();
    pending.sender.send(Reply::Saved).unwrap();
    let out = paste(&dest, &pending.data, &Job::detached(Resolve::KeepBoth));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(std::fs::read(dest.join("same.txt")).unwrap(), b"old", "the existing file is kept");
    let made = &out.items[0].1;
    assert_eq!(std::fs::read(dest.join(std::str::from_utf8(made.file_name()).unwrap())).unwrap(), b"new");
    assert_ne!(made.file_name(), b"same.txt");
}

#[test]
fn a_source_that_declines_falls_back_to_the_ordinary_dropped_data() {
    let t = Temp::new("xds-declined");
    let fallback = DroppedFile { name: b"page.desktop".to_vec(), bytes: b"[Desktop Entry]\n".to_vec() };
    let pending = prepare(&offer("page.html"), Some(fallback), t.path()).unwrap();
    pending.sender.send(Reply::Declined).unwrap();
    let out = paste(t.path(), &pending.data, &Job::detached(Resolve::Skip));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(std::fs::read(t.path().join("page.desktop")).unwrap(), b"[Desktop Entry]\n");
    assert!(!t.path().join("page.html").exists());
}

#[test]
fn a_source_that_declines_without_other_data_or_fails_shows_an_error() {
    for answer in [Reply::Declined, Reply::Failed] {
        let t = Temp::new("xds-failed");
        let pending = prepare(&offer("x.txt"), None, t.path()).unwrap();
        pending.sender.send(answer).unwrap();
        let out = paste(t.path(), &pending.data, &Job::detached(Resolve::Skip));
        assert_eq!(out.error.map(|e| e.message).as_deref(), Some(S::UiDroppedSaveFailed.t()), "{answer:?}");
        assert!(out.items.is_empty());
        assert!(!t.path().join("x.txt").exists());
    }
}

#[test]
fn a_source_that_never_answers_ends_in_an_error_after_the_wait() {
    let t = Temp::new("xds-silent");
    let pending = prepare(&offer("x.txt"), None, t.path()).unwrap();
    let staged = staged_path(&pending.uri);
    let promise = take(&pending.data).unwrap();
    let started = Instant::now();
    let out = settle(&fs(t.path()), promise, &Job::detached(Resolve::Skip), Duration::from_millis(150));
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert_eq!(out.error.map(|e| e.message).as_deref(), Some(S::UiDroppedSaveSilent.t()));
    assert!(!staged.parent().unwrap().exists());
    drop(pending.sender);
}

#[test]
fn a_window_that_stops_waiting_counts_as_no_answer_and_cancel_stops_the_wait() {
    let t = Temp::new("xds-gone");
    let pending = prepare(&offer("x.txt"), None, t.path()).unwrap();
    drop(pending.sender);
    let out = paste(t.path(), &pending.data, &Job::detached(Resolve::Skip));
    assert_eq!(out.error.map(|e| e.message).as_deref(), Some(S::UiDroppedSaveSilent.t()));
    let waiting = prepare(&offer("y.txt"), None, t.path()).unwrap();
    let job = Job::detached(Resolve::Skip);
    job.ctl().cancel();
    let started = Instant::now();
    let out = paste(t.path(), &waiting.data, &job);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(out.error.map(|e| e.message).as_deref(), Some(flit_platform::CANCELLED));
    drop(waiting.sender);
}

#[test]
fn ordinary_dropped_data_with_the_same_name_is_not_mistaken_for_a_promised_file() {
    let t = Temp::new("xds-ordinary");
    let pending = prepare(&offer("note.txt"), None, t.path()).unwrap();
    let ordinary = Arc::new(DroppedFile { name: b"note.txt".to_vec(), bytes: b"typed".to_vec() });
    let out = paste(t.path(), &ordinary, &Job::detached(Resolve::Skip));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(std::fs::read(t.path().join("note.txt")).unwrap(), b"typed");
    forget(&pending.data);
}

#[test]
fn direct_save_without_other_types_fetches_nothing_and_ordinary_drops_fetch_as_before() {
    use winit::data_transfer::TypeHint;
    let png = crate::dropped::PNG;
    assert_eq!(crate::dropped::fetched(&[], true), Vec::<TypeHint>::new(), "a source offering only direct save is not asked for a file list");
    assert_eq!(crate::dropped::fetched(&[], false), [TypeHint::UriList]);
    let browser = [TypeHint::Plaintext, png, TypeHint::UriList];
    assert_eq!(crate::dropped::fetched(&browser, true), crate::dropped::fetched(&browser, false));
    assert_eq!(crate::dropped::fetched(&browser, false), [TypeHint::UriList, png, TypeHint::Plaintext]);
}

#[test]
fn the_x11_link_reads_the_offered_name_and_writes_the_uri_on_the_source_window() {
    use x11rb::protocol::xproto::{CreateWindowAux, WindowClass};
    let Some(display) = std::env::var("FLIT_TEST_XVFB_DISPLAY").ok().filter(|d| !d.is_empty()) else {
        eprintln!("no FLIT_TEST_XVFB_DISPLAY with a private Xvfb, skipped");
        return;
    };
    let display = std::ffi::CString::new(display).unwrap();
    let (source, screen) = XCBConnection::connect(Some(&display)).unwrap();
    let root = source.setup().roots[screen].root;
    let link = X11::connect(Some(display.as_c_str())).unwrap();
    let window = source.generate_id().unwrap();
    source.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1)).unwrap();
    source.change_property8(PropMode::REPLACE, window, link.direct_save, link.text, b"report 1.pdf").unwrap();
    source.flush().unwrap();
    let mut found = None;
    for _ in 0..50 {
        source.set_selection_owner(window, link.selection, x11rb::CURRENT_TIME).unwrap();
        source.get_input_focus().unwrap().reply().unwrap();
        found = probe(&link, &[link.direct_save]);
        if found.as_ref().is_some_and(|f| f.window == window) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(found, Some(Offer { window, name: b"report 1.pdf".to_vec(), atom: link.direct_save }));
    assert_eq!(probe(&link, &[]), None);
    assert!(link.write_uri(window, b"file:///tmp/x/report%201.pdf"));
    let stored = source.get_property(false, window, link.direct_save, AtomEnum::ANY, 0, 1024).unwrap().reply().unwrap();
    assert_eq!((stored.type_, stored.format, stored.value.as_slice()), (link.text, 8, &b"file:///tmp/x/report%201.pdf"[..]));
    source.destroy_window(window).unwrap();
    source.flush().unwrap();
}

#[test]
fn a_folder_saved_by_the_source_is_copied_whole_and_nothing_stays_in_the_private_folder() {
    let t = Temp::new("xds-folder");
    let dest = t.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let pending = prepare(&offer("photos"), None, t.path()).unwrap();
    let staged = staged_path(&pending.uri);
    std::fs::create_dir_all(staged.join("2024")).unwrap();
    std::fs::write(staged.join("2024").join("a.jpg"), b"jpeg").unwrap();
    std::fs::write(staged.parent().unwrap().join("extra.tmp"), b"left by the source").unwrap();
    pending.sender.send(Reply::Saved).unwrap();
    let out = paste(&dest, &pending.data, &Job::detached(Resolve::Skip));
    assert!(out.error.is_none(), "{:?}", out.error);
    let target = fs(&dest.join("photos"));
    assert_eq!(out.items, [(target.clone(), target)]);
    assert_eq!(std::fs::read(dest.join("photos").join("2024").join("a.jpg")).unwrap(), b"jpeg");
    assert!(!staged.parent().unwrap().exists(), "the whole private folder is removed, with whatever the source left in it");
}

#[test]
fn a_wait_replaced_by_a_new_drag_is_cancelled_without_an_error() {
    let t = Temp::new("xds-abandoned");
    let pending = prepare(&offer("x.txt"), None, t.path()).unwrap();
    let staged = staged_path(&pending.uri);
    std::fs::write(&staged, b"half").unwrap();
    pending.sender.send(Reply::Abandoned).unwrap();
    let out = paste(t.path(), &pending.data, &Job::detached(Resolve::Skip));
    assert_eq!(out.error.map(|e| e.message).as_deref(), Some(flit_platform::CANCELLED), "a cancelled operation is not reported as a failure");
    assert!(!t.path().join("x.txt").exists());
    assert!(!staged.parent().unwrap().exists());
}

#[test]
fn only_old_private_save_folders_of_this_user_are_swept_at_start() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let t = Temp::new("xds-sweep");
    let root = t.path();
    let uid = unsafe { libc::geteuid() };
    let make = |name: &str, mode: u32| {
        let dir = root.join(name);
        std::fs::DirBuilder::new().mode(mode).create(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
        std::fs::write(dir.join("inner"), b"x").unwrap();
        dir
    };
    let age = |dir: &Path| {
        let old = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        std::fs::File::open(dir).unwrap().set_modified(old).unwrap();
    };
    let stale = make("flit-save-1-0-0", 0o700);
    age(&stale);
    let fresh = make("flit-save-1-1-0", 0o700);
    let open = make("flit-save-1-2-0", 0o755);
    age(&open);
    let other = make("flit-drop-1-0-0", 0o700);
    age(&other);
    let target = make("target", 0o700);
    age(&target);
    std::os::unix::fs::symlink(&target, root.join("flit-save-1-3-0")).unwrap();
    sweep(root, STALE, uid.wrapping_add(1));
    assert!(stale.exists(), "folders of another user are left alone");
    sweep(root, STALE, uid);
    assert!(!stale.exists(), "an old private folder is removed");
    assert!(fresh.exists(), "a folder of a running save is kept");
    assert!(open.exists() && other.exists(), "folders that are not private save folders are kept");
    assert!(target.join("inner").exists(), "a link is never followed");
}
