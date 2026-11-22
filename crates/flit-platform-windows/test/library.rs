use super::*;
use flit_core::listing::attr;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use windows::core::{implement, Ref, Result as ComResult, PWSTR};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL, E_INVALIDARG, E_NOTIMPL};
use windows::Win32::UI::Shell::{IShellLibrary_Impl, DEFAULTSAVEFOLDERTYPE, LIBRARYFOLDERFILTER, LIBRARYOPTIONFLAGS, LIBRARYSAVEFLAGS, LSF_OVERRIDEEXISTING, SIGDN_URL};

const SHELL_LIBRARY_TIMEOUT: Duration = Duration::from_secs(60);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let root = crate::test_temp().join(format!("flit-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn at(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_library(dir: &Path, name: &str) -> FsPath {
    com_sta();
    unsafe {
        let lib: IShellLibrary = CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER).unwrap();
        let dir = FsPath::parse(dir.to_str().unwrap()).unwrap();
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        path_of(&lib.Save(&item(&dir).unwrap(), PCWSTR(wide.as_ptr()), LSF_OVERRIDEEXISTING).unwrap()).unwrap()
    }
}

fn within_shell_timeout(f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        com_sta();
        let _ = tx.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    });
    match rx.recv_timeout(SHELL_LIBRARY_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("library steps failed"),
        Err(_) => panic!("a shell library call made no progress for {} s, the shell is in a bad state", SHELL_LIBRARY_TIMEOUT.as_secs()),
    }
}

fn library_steps() {
    let t = Temp::new("ops");
    for d in ["one", "two", "libs"] {
        std::fs::create_dir_all(t.0.join(d)).unwrap();
    }
    std::fs::write(t.0.join(r"one\a.txt"), "a").unwrap();
    std::fs::create_dir(t.0.join(r"two\sub")).unwrap();
    let lib = temp_library(&t.0.join("libs"), "Test");
    assert!(flit_core::library::is_library(&lib));
    let (one, two) = (t.at("one"), t.at("two"));
    apply(&LibraryOp::Add { library: lib.clone(), folder: one.clone() }).unwrap();
    apply(&LibraryOp::Add { library: lib.clone(), folder: two.clone() }).unwrap();
    let read_back = read(&lib).unwrap();
    assert_eq!(read_back.folders, [one.clone(), two.clone()]);
    assert_eq!(read_back.name, "Test");
    apply(&LibraryOp::SetSave { library: lib.clone(), folder: two.clone() }).unwrap();
    assert_eq!(read(&lib).unwrap().save, Some(two.clone()));
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    NtFs.read_dir(&lib, &mut b).unwrap();
    let l = b.listing;
    let mut rows: Vec<(String, String, bool)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), String::from_utf8_lossy(library::folder(&l, i)).into_owned(), l.entries[i as usize].attrs & attr::DIR != 0)).collect();
    rows.sort();
    assert_eq!(rows, [("a.txt".to_owned(), one.display().into_owned(), false), ("sub".to_owned(), two.display().into_owned(), true)]);
    apply(&LibraryOp::Remove { library: lib.clone(), folder: one.clone() }).unwrap();
    assert_eq!(folders(&lib), [two]);
    assert!(apply(&LibraryOp::Add { library: t.at(r"libs\missing.library-ms"), folder: one }).is_err());
}

#[test]
#[ignore = "Requires FLIT_SHELL_LIBRARY_ALLOW_INDEXING=1 in an isolated Windows integration account"]
fn a_temp_library_gains_and_loses_folders_and_lists_them_merged() {
    assert!(matches!(std::env::var("FLIT_SHELL_LIBRARY_ALLOW_INDEXING").as_deref(), Ok("1")), "This test changes the Windows Search index. Set FLIT_SHELL_LIBRARY_ALLOW_INDEXING=1 only in a dedicated Windows integration test account.");
    within_shell_timeout(library_steps);
}

#[derive(Clone, Debug, PartialEq)]
enum Edit {
    Add(FsPath),
    Remove(FsPath),
    Save(DEFAULTSAVEFOLDERTYPE, FsPath),
    Commit,
}

#[derive(Default)]
struct Edits {
    calls: Vec<Edit>,
    deny_edit: bool,
    deny_commit: bool,
}

#[implement(IShellLibrary)]
struct TestLibrary(Rc<RefCell<Edits>>);

impl TestLibrary {
    fn edit(&self, call: Edit) -> ComResult<()> {
        let mut edits = self.0.borrow_mut();
        edits.calls.push(call);
        if edits.deny_edit {
            Err(E_ACCESSDENIED.into())
        } else {
            Ok(())
        }
    }
}

impl IShellLibrary_Impl for TestLibrary_Impl {
    fn AddFolder(&self, item: Ref<IShellItem>) -> ComResult<()> {
        self.edit(Edit::Add(path_of(item.as_ref().ok_or(E_INVALIDARG)?).ok_or(E_INVALIDARG)?))
    }

    fn RemoveFolder(&self, item: Ref<IShellItem>) -> ComResult<()> {
        self.edit(Edit::Remove(path_of(item.as_ref().ok_or(E_INVALIDARG)?).ok_or(E_INVALIDARG)?))
    }

    fn SetDefaultSaveFolder(&self, kind: DEFAULTSAVEFOLDERTYPE, item: Ref<IShellItem>) -> ComResult<()> {
        self.edit(Edit::Save(kind, path_of(item.as_ref().ok_or(E_INVALIDARG)?).ok_or(E_INVALIDARG)?))
    }

    fn Commit(&self) -> ComResult<()> {
        let mut edits = self.0.borrow_mut();
        edits.calls.push(Edit::Commit);
        if edits.deny_commit {
            Err(E_FAIL.into())
        } else {
            Ok(())
        }
    }

    fn LoadLibraryFromItem(&self, _: Ref<IShellItem>, _: u32) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn LoadLibraryFromKnownFolder(&self, _: *const GUID, _: u32) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetFolders(&self, _: LIBRARYFOLDERFILTER, _: *const GUID, _: *mut *mut core::ffi::c_void) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn ResolveFolder(&self, _: Ref<IShellItem>, _: u32, _: *const GUID, _: *mut *mut core::ffi::c_void) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetDefaultSaveFolder(&self, _: DEFAULTSAVEFOLDERTYPE, _: *const GUID, _: *mut *mut core::ffi::c_void) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetOptions(&self) -> ComResult<LIBRARYOPTIONFLAGS> {
        Err(E_NOTIMPL.into())
    }
    fn SetOptions(&self, _: LIBRARYOPTIONFLAGS, _: LIBRARYOPTIONFLAGS) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetFolderType(&self) -> ComResult<GUID> {
        Err(E_NOTIMPL.into())
    }
    fn SetFolderType(&self, _: *const GUID) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetIcon(&self) -> ComResult<PWSTR> {
        Err(E_NOTIMPL.into())
    }
    fn SetIcon(&self, _: &PCWSTR) -> ComResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn Save(&self, _: Ref<IShellItem>, _: &PCWSTR, _: LIBRARYSAVEFLAGS) -> ComResult<IShellItem> {
        Err(E_NOTIMPL.into())
    }
    fn SaveInKnownFolder(&self, _: *const GUID, _: &PCWSTR, _: LIBRARYSAVEFLAGS) -> ComResult<IShellItem> {
        Err(E_NOTIMPL.into())
    }
}

#[test]
fn library_edits_commit_the_selected_folders_without_the_system_index() {
    within_shell_timeout(|| {
        let temp = Temp::new("edits");
        for dir in ["one", "two"] {
            std::fs::create_dir(temp.0.join(dir)).unwrap();
        }
        let library = temp.at("Test.library-ms");
        let edits = Rc::new(RefCell::new(Edits::default()));
        let fake: IShellLibrary = TestLibrary(edits.clone()).into();
        let open = |path: &FsPath, write: bool| {
            assert_eq!(path, &library);
            assert!(write);
            Ok(fake.clone())
        };
        let (one, two) = (temp.at("one"), temp.at("two"));
        apply_with(&LibraryOp::Add { library: library.clone(), folder: one.clone() }, open).unwrap();
        apply_with(&LibraryOp::Add { library: library.clone(), folder: two.clone() }, open).unwrap();
        apply_with(&LibraryOp::SetSave { library: library.clone(), folder: two.clone() }, open).unwrap();
        apply_with(&LibraryOp::Remove { library: library.clone(), folder: one.clone() }, open).unwrap();
        assert_eq!(edits.borrow().calls, [Edit::Add(one.clone()), Edit::Commit, Edit::Add(two.clone()), Edit::Commit, Edit::Save(DSFT_DETECT, two), Edit::Commit, Edit::Remove(one), Edit::Commit]);
    });
}

#[test]
fn library_edits_report_open_edit_and_commit_failures_and_refuse_missing_folders() {
    within_shell_timeout(|| {
        let temp = Temp::new("edit-errors");
        std::fs::create_dir(temp.0.join("one")).unwrap();
        let (library, folder) = (temp.at("Test.library-ms"), temp.at("one"));
        let edits = Rc::new(RefCell::new(Edits::default()));
        let fake: IShellLibrary = TestLibrary(edits.clone()).into();
        let open = |_: &FsPath, _: bool| Ok(fake.clone());
        let ops = [LibraryOp::Add { library: library.clone(), folder: folder.clone() }, LibraryOp::Remove { library: library.clone(), folder: folder.clone() }, LibraryOp::SetSave { library: library.clone(), folder: folder.clone() }];
        for op in &ops {
            let error = apply_with(op, |_, _| Err(E_ACCESSDENIED.into())).unwrap_err();
            assert_eq!(error.code, i64::from(E_ACCESSDENIED.0));
            assert!(edits.borrow().calls.is_empty());
            edits.borrow_mut().deny_edit = true;
            let error = apply_with(op, open).unwrap_err();
            assert_eq!(error.code, i64::from(E_ACCESSDENIED.0));
            assert_eq!(edits.borrow().calls.len(), 1, "a rejected edit must not be committed");
            assert!(!edits.borrow().calls.contains(&Edit::Commit));
            edits.borrow_mut().calls.clear();
            edits.borrow_mut().deny_edit = false;
            edits.borrow_mut().deny_commit = true;
            let error = apply_with(op, open).unwrap_err();
            assert_eq!(error.code, i64::from(E_FAIL.0));
            assert_eq!(edits.borrow().calls.len(), 2);
            assert_eq!(edits.borrow().calls.last(), Some(&Edit::Commit));
            edits.borrow_mut().calls.clear();
            edits.borrow_mut().deny_commit = false;
        }
        for op in [LibraryOp::Add { library: library.clone(), folder: temp.at("missing") }, LibraryOp::Remove { library: library.clone(), folder: temp.at("missing") }, LibraryOp::SetSave { library, folder: temp.at("missing") }] {
            assert!(apply_with(&op, open).is_err());
            assert!(edits.borrow().calls.is_empty(), "an invalid shell item must not reach an edit or commit");
        }
    });
}

#[test]
fn fixture_libraries_read_and_list_real_folders_without_indexing_them() {
    within_shell_timeout(|| {
        let temp = Temp::new("fixture");
        for dir in ["one & папка", "two"] {
            std::fs::create_dir(temp.0.join(dir)).unwrap();
        }
        std::fs::write(temp.0.join("one & папка/a.txt"), "a").unwrap();
        std::fs::create_dir(temp.0.join("two/sub")).unwrap();
        let empty = temp_library(&temp.0, "Empty");
        let empty = read(&empty).unwrap();
        assert!(empty.folders.is_empty());
        assert!(empty.save.is_none());
        let (one, two) = (temp.at("one & папка"), temp.at("two"));
        let connectors: String = [&one, &two]
            .into_iter()
            .enumerate()
            .map(|(i, folder)| {
                let url = unsafe { take_pwstr(item(folder).unwrap().GetDisplayName(SIGDN_URL).unwrap()).unwrap() };
                let url = String::from_utf16_lossy(&url).replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                format!("<searchConnectorDescription><isDefaultSaveLocation>{}</isDefaultSaveLocation><simpleLocation><url>{url}</url></simpleLocation></searchConnectorDescription>", i == 1)
            })
            .collect();
        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><libraryDescription xmlns=\"http://schemas.microsoft.com/windows/2009/library\"><name>Test</name><version>6</version><searchConnectorDescriptionList>{connectors}</searchConnectorDescriptionList></libraryDescription>");
        std::fs::write(temp.0.join("Test.library-ms"), xml).unwrap();
        let lib = temp.at("Test.library-ms");
        let back = read(&lib).unwrap();
        assert_eq!(back.name, "Test");
        assert_eq!(back.folders, [one.clone(), two.clone()]);
        assert_eq!(back.save, Some(two.clone()));
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut out = ListingBuilder::new(CaseMode::Insensitive, &mut go);
        NtFs.read_dir(&lib, &mut out).unwrap();
        let listing = out.listing;
        let mut rows: Vec<_> = (0..listing.len() as u32).map(|i| (listing.name(i).to_vec(), library::folder(&listing, i).to_vec(), listing.entries[i as usize].attrs & attr::DIR != 0)).collect();
        rows.sort();
        assert_eq!(rows, [(b"a.txt".to_vec(), one.display().as_bytes().to_vec(), false), (b"sub".to_vec(), two.display().as_bytes().to_vec(), true)]);
        assert!(read(&temp.at("missing.library-ms")).is_none());
        assert!(folders(&temp.at("missing.library-ms")).is_empty());
        assert!(list(&temp.at("missing.library-ms"), &mut ListingBuilder::new(CaseMode::Insensitive, &mut go)).is_err());
    });
}

fn restoring_steps() {
    let t = Temp::new("restore");
    std::fs::write(t.0.join("Music.library-ms"), "kept").unwrap();
    assert_eq!(restore(&t.0, &DEFAULTS).unwrap(), 3);
    assert_eq!(std::fs::read_to_string(t.0.join("Music.library-ms")).unwrap(), "kept", "existing libraries are never overwritten");
    assert_eq!(restore(&t.0, &DEFAULTS).unwrap(), 0);
    let docs = read(&t.at("Documents.library-ms")).expect("the shell reads the restored library");
    assert_eq!(docs.folders.len(), 1);
    assert_eq!(docs.save.as_ref(), docs.folders.first());
}

#[test]
fn restoring_defaults_writes_only_missing_classic_libraries_that_the_shell_can_read() {
    within_shell_timeout(restoring_steps);
}
