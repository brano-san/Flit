use super::*;
use crate::instance::user_sid;
use crate::pipe::{connect_to, listen_at, PipeConn};
use flit_platform::OnClient;
use flit_search::engine::Engine;
use flit_search::index::{IndexRecord, IndexSink};
use flit_search::proto::{Sort, SortKey};
use flit_search::query::{parse, Clock};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

struct Tree {
    base: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("flit-access-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("open")).unwrap();
        std::fs::create_dir_all(base.join("locked")).unwrap();
        let me = user_sid().unwrap();
        let sddl: Vec<u16> = format!("D:P(A;;0x1F01FE;;;{me})").encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }.unwrap();
        let locked = wide(&base.join("locked"));
        assert!(unsafe { SetFileSecurityW(PCWSTR(locked.as_ptr()), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, sd) }.as_bool());
        std::fs::create_dir(base.join("locked").join("inner")).unwrap();
        assert!(std::fs::read_dir(base.join("locked")).is_err(), "the folder is not listable");
        Self { base }
    }

    fn index(&self) -> VolumeIndex {
        let root = self.base.to_str().unwrap().as_bytes().to_vec();
        let mut v = VolumeIndex::new(self.base.to_str().unwrap(), &root, b'\\');
        let put = |v: &mut VolumeIndex, id: u64, parent: u64, name: &str, dir: bool| {
            let attrs = if dir { flit_core::attr::DIR } else { 0 };
            v.upsert(&IndexRecord { id, parent, name: name.as_bytes(), size: 1, modified: 0, attrs });
        };
        put(&mut v, 1, 1, ".", true);
        put(&mut v, 10, 1, "open", true);
        put(&mut v, 11, 1, "locked", true);
        put(&mut v, 12, 10, "seen.txt", false);
        put(&mut v, 13, 11, "secret.txt", false);
        put(&mut v, 14, 11, "inner", true);
        put(&mut v, 15, 14, "deep.txt", false);
        put(&mut v, 16, 1, "top.txt", false);
        v.commit();
        v
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(self.base.join("locked").join("inner"));
        let _ = std::fs::remove_dir(self.base.join("locked"));
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn wide(p: &Path) -> Vec<u16> {
    p.to_str().unwrap().encode_utf16().chain(Some(0)).collect()
}

fn own_token() -> TokenAccess {
    let mut process = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut process) }.unwrap();
    let process = Handle(process);
    let mut dup = HANDLE::default();
    unsafe { DuplicateToken(process.0, SecurityIdentification, &mut dup) }.unwrap();
    TokenAccess::new(Handle(dup))
}

fn visible(v: &VolumeIndex, access: &dyn Visibility) -> Vec<String> {
    let e = Engine { volumes: vec![v.clone()], version: 1 };
    let q = parse("", Clock { now: 0, utc_offset_min: 0 }).unwrap();
    let mut rs = e.search_as(&q, Sort { key: SortKey::Name, descending: false }, Some(access), &|| false).unwrap();
    let (batch, _) = e.rows(&mut rs, 0, 100);
    batch.entries(true).unwrap().map(|(n, _)| String::from_utf8_lossy(n).into_owned()).collect()
}

#[test]
fn entries_of_folders_the_client_cannot_list_are_hidden() {
    let tree = Tree::new("token");
    let v = tree.index();
    let access = own_token();
    let dir = |id: u64| v.lookup(id).unwrap();
    assert!(access.can_list(&v, NONE));
    assert!(access.can_list(&v, dir(10)));
    assert!(!access.can_list(&v, dir(11)));
    assert!(access.can_list(&v, dir(14)), "traverse checking is bypassed, only the parent counts");
    assert!(!access.can_list(&v, dir(11)), "cached answers stay the same");
    assert_eq!(visible(&v, &access), ["deep.txt", "locked", "open", "seen.txt", "top.txt"], "inner sits in the locked folder, deep.txt does not");
}

#[test]
fn the_pipe_client_token_drives_the_check() {
    let tree = Tree::new("pipe");
    let v = Arc::new(tree.index());
    let name = format!(r"\\.\pipe\flit-test-access-{}", std::process::id());
    let (server_v, server_name) = (v.clone(), name.clone());
    std::thread::spawn(move || {
        let on_client: OnClient<PipeConn> = Arc::new(move |mut c: PipeConn| {
            let mut hello = [0u8; 1];
            if c.read_exact(&mut hello).is_err() {
                return;
            }
            let answer = match c.client_access() {
                Ok(a) => [a.can_list(&server_v, server_v.lookup(10).unwrap()) as u8, a.can_list(&server_v, server_v.lookup(11).unwrap()) as u8],
                Err(_) => [9, 9],
            };
            let _ = c.write_all(&answer);
        });
        let _ = listen_at(&server_name, 2, on_client);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut c = loop {
        match connect_to(&name) {
            Ok(c) => break c,
            Err(e) => assert!(std::time::Instant::now() < deadline, "{e}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    c.write_all(&[1]).unwrap();
    let mut answer = [0u8; 2];
    c.read_exact(&mut answer).unwrap();
    assert_eq!(answer, [1, 0], "identification-level impersonation is enough for AccessCheck");
    assert!(flit_platform::Duplex::access(&c).unwrap().is_none(), "nothing is filtered outside the service account");
}
