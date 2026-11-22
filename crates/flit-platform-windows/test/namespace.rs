use super::*;

#[test]
#[ignore]
fn a_public_ftp_folder_lists_and_copies_a_file_through_windows_shell() {
    use flit_core::provider::{FileOp, OpCtl};
    use flit_platform::{FileOps, Job, Shell};

    let mut shell = crate::shell::ShellCom::new().unwrap();
    let root = shell.connect("ftp://ftp.gnu.org/").unwrap();
    let source = root.join(b"MISSING-FILES.README");
    let mut on_batch = |_: &ListingBuilder<'_>| true;
    let mut listing = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch);
    read_dir(&root, &mut listing).unwrap();
    assert!((0..listing.listing.len() as u32).any(|i| listing.listing.name(i) == source.file_name()));
    let data = crate::context_menu::Pidls::parse_for_ui(std::slice::from_ref(&source)).unwrap();
    let _: windows::Win32::System::Com::IDataObject = data.ui_object(Default::default()).unwrap();

    let dir = crate::test_temp().join(format!("flit-ftp-probe-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let dest = FsPath::parse(&dir.to_string_lossy()).unwrap();
    let expected = (source.clone(), dest.join(source.file_name()));
    let job = Job::new(OpCtl::default(), Box::new(|_| {}), Box::new(|_| {}));
    let out = shell.file_op(&FileOp::Copy { items: vec![source], dest }, &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, [expected]);
    let file = dir.join("MISSING-FILES.README");
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.is_empty() && bytes.len() < 10 * 1024);
    println!("{} entries listed; copied {} bytes to {}", listing.listing.len(), bytes.len(), file.display());
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
use flit_core::sort::CaseMode;
use std::cell::Cell;

#[derive(Clone)]
struct Fake {
    name: &'static str,
    kids: Vec<Fake>,
    size: u64,
}

thread_local! {
    static OPENED: Cell<usize> = const { Cell::new(0) };
}

fn dir(name: &'static str, kids: Vec<Fake>) -> Fake {
    Fake { name, kids, size: 0 }
}

fn file(name: &'static str, size: u64) -> Fake {
    Fake { name, kids: Vec::new(), size }
}

impl Node for Fake {
    fn name(&self) -> Vec<u8> {
        self.name.as_bytes().to_vec()
    }
    fn meta(&self) -> Meta {
        Meta { size: self.size, attrs: if self.size == 0 { attr::DIR } else { 0 }, ..Default::default() }
    }
    fn children(&self, each: &mut dyn FnMut(Self) -> bool) -> Result<(), FsError> {
        if self.name == "Locked" {
            return Err(FsError { kind: FsErrorKind::AccessDenied, code: 5 });
        }
        for k in &self.kids {
            OPENED.set(OPENED.get() + 1);
            if !each(k.clone()) {
                break;
            }
        }
        Ok(())
    }
}

fn phone() -> Fake {
    let photos: Vec<Fake> = (0..100).map(|k| file(Box::leak(format!("IMG_{k:04}.jpg").into_boxed_str()), 1000 + k)).collect();
    dir("This PC", vec![dir("Pixel 7", vec![dir("Internal shared storage", vec![dir("DCIM", vec![dir("Camera", photos)]), dir("Locked", vec![])])])])
}

#[test]
fn walk_matches_names_without_case_and_stops_at_the_hit() {
    OPENED.set(0);
    let camera = walk(phone(), &[b"pixel 7", b"Internal shared storage", b"dcim", b"Camera"]).unwrap();
    assert_eq!(camera.kids.len(), 100);
    assert_eq!(OPENED.get(), 4);
    assert_eq!(walk(phone(), &[b"Pixel 7", b"SD card"]).err(), Some(not_found()));
}

#[test]
fn listing_streams_in_batches_and_stops_when_cancelled() {
    let camera = walk(phone(), &[b"Pixel 7", b"Internal shared storage", b"DCIM", b"Camera"]).unwrap();
    let mut batches = 0;
    let mut on_batch = |_: &ListingBuilder<'_>| {
        batches += 1;
        true
    };
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch);
    list(&camera, &mut b, |_, _| {}).unwrap();
    assert_eq!(b.listing.len(), 100);
    drop(b);
    assert_eq!(batches, 4);
    OPENED.set(0);
    let mut stop = |_: &ListingBuilder<'_>| false;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut stop);
    list(&camera, &mut b, |_, _| {}).unwrap();
    assert_eq!((b.listing.len(), OPENED.get()), (BATCH, BATCH));
}

#[test]
fn enumeration_errors_reach_the_listing() {
    let storage = walk(phone(), &[b"Pixel 7", b"Internal shared storage"]).unwrap();
    let locked = walk(storage, &[b"Locked"]).unwrap();
    let mut on_batch = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch);
    assert_eq!(list(&locked, &mut b, |_, _| {}).err().map(|e| e.kind), Some(FsErrorKind::AccessDenied));
}

#[test]
fn shell_next_errors_end_the_listing_and_ftp_next_errors_are_reported() {
    let error = FsError { kind: FsErrorKind::Unavailable, code: 0x80004005 };
    for first in [None, Some("server or device")] {
        for ftp in [false, true] {
            let mut row = first;
            let mut shown = Vec::new();
            let outcome = enumerate(ftp, || row.take().map_or(Err(error.clone()), |row| Ok(Some(row))), &mut |item| {
                shown.push(item);
                true
            });
            assert_eq!(shown, first.into_iter().collect::<Vec<_>>());
            assert_eq!(outcome, if ftp { Err(error.clone()) } else { Ok(()) });
        }
    }
}

#[test]
fn network_root_resolves_through_the_real_shell() {
    let net = Item { raw: item(&FsPath::network()).expect("the Network folder exists on every Windows"), ftp: false };
    assert!(net.meta().attrs & attr::DIR != 0);
    assert_eq!(item(&FsPath::device(b"No Such Phone 123").unwrap()).err(), Some(not_found()));
}

#[test]
fn ftp_actions_use_the_listed_pidl_on_another_thread() {
    let root = FsPath::parse("ftp://flit-cache.invalid/pub/").unwrap();
    let selected = root.join(b"flit.exe");
    let missing = root.join(b"unlisted.exe");
    let expected = std::env::current_exe().unwrap();
    let local = FsPath::parse(expected.to_str().unwrap()).unwrap();
    let local = item(&local).unwrap();
    remember_folder(&root, &local).remember(selected.file_name(), &local);
    std::thread::spawn(move || {
        use crate::context_menu::Pidls;
        assert!(Pidls::parse_for_ui(&[selected]).is_ok());
        assert!(Pidls::parse_for_ui(&[root]).is_ok());
        assert_eq!(Pidls::parse_for_ui(&[missing]).err().unwrap().code, not_found().code);
    }).join().unwrap();
}

#[test]
fn portable_devices_are_told_apart_by_their_parsing_name() {
    assert!(is_device(r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\\?\usb#vid_18d1&pid_4ee1#abc#{6ac27878-a6fa-4155-ba85-f98f491d4f33}"));
    assert!(!is_device(r"C:\"));
    assert!(!is_device(r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}"));
    let d = device_drive(FsPath::device(b"Pixel 7").unwrap(), "Pixel 7".into());
    assert_eq!((d.title(), d.path.display().into_owned()), ("Pixel 7".to_owned(), r"This PC\Pixel 7".to_owned()));
    assert!(devices().iter().all(|d| d.path.is_device()));
}
