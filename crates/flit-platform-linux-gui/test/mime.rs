use super::*;

#[test]
fn globs_pick_the_heaviest_simple_extension() {
    let mut map = HashMap::new();
    parse_globs("# comment\n50:text/plain:*.txt\n80:text/x-rust:*.rs\n40:text/x-other:*.rs\n50:text/x-readme:README\n50:x/y:*.[ch]\n60:image/jpeg:*.JPG\n", &mut map);
    assert_eq!(map.get("rs").map(|v| v.1.as_str()), Some("text/x-rust"));
    assert_eq!(map.get("jpg").map(|v| v.1.as_str()), Some("image/jpeg"));
    assert!(!map.contains_key("README") && !map.contains_key("[ch]"));
}

#[test]
fn the_comment_without_a_language_names_the_type() {
    let xml = r#"<mime-type type="text/x-rust"><comment xml:lang="de">Rust-Quelltext</comment><comment>Rust source &amp; code</comment></mime-type>"#;
    assert_eq!(parse_comment(xml).as_deref(), Some("Rust source & code"));
    assert_eq!(parse_comment("<mime-type/>"), None);
}

#[test]
fn installed_types_have_names_when_shared_mime_info_is_present() {
    if !std::path::Path::new("/usr/share/mime/globs2").exists() {
        return;
    }
    assert_eq!(type_name(b"PNG").as_deref(), Some("PNG image"));
    assert_eq!(type_name(b"no-such-extension-here"), None);
}

#[test]
fn lineage_walks_subclasses_and_text_falls_back_to_plain_text_but_never_to_octet_stream() {
    let mut map = HashMap::new();
    parse_subclasses("application/x-shellscript application/x-executable\napplication/x-shellscript text/plain\ntext/x-rust text/x-csrc\napplication/x-executable application/octet-stream\n", &mut map);
    let parents = |m: &str| map.get(m).cloned().unwrap_or_default();
    assert_eq!(lineage("application/x-shellscript", parents), ["application/x-shellscript", "application/x-executable", "text/plain"]);
    assert_eq!(lineage("text/x-rust", parents), ["text/x-rust", "text/x-csrc", "text/plain"]);
    assert_eq!(lineage("image/png", parents), ["image/png"]);
}

#[test]
fn sniffing_tells_text_from_binary_even_when_a_character_is_cut_at_the_end() {
    assert_eq!(sniff(b"all: build\n"), "text/plain");
    assert_eq!(sniff(&"привет".as_bytes()[..5]), "text/plain");
    assert_eq!(sniff(b""), "text/plain");
    assert_eq!(sniff(b"\x7fELF\x02\x01\x01\0"), OCTET_STREAM);
    assert_eq!(sniff(b"\xff\xfe\xfa"), OCTET_STREAM);
}

#[test]
fn content_magic_uses_system_mime_data_and_keeps_the_old_fallback_without_gio() {
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0";
    if content_api().is_some() {
        assert_eq!(content_mime(png).as_deref(), Some("image/png"));
    }
    assert_eq!(sniff(b"hello world\n"), "text/plain");
    assert_eq!(sniff(b"\0binary"), OCTET_STREAM);
}

#[test]
fn content_cache_is_bounded_and_a_new_mtime_reads_again() {
    let mut cache = ContentCache::default();
    let key = |n, stamp| ContentKey { path: PathBuf::from(format!("/tmp/{n}")), modified: (stamp, 0), size: 20 };
    let a = key(1, 1);
    cache.insert(a.clone(), "image/png".into());
    assert_eq!(cache.get(&a).map(String::as_str), Some("image/png"));
    assert!(cache.get(&key(1, 2)).is_none());
    for i in 0..CONTENT_CACHE_LIMIT + 10 {
        cache.insert(key(i, 1), OCTET_STREAM.into());
    }
    assert_eq!(cache.entries.len(), CONTENT_CACHE_LIMIT);
}

#[test]
fn content_reader_limits_regular_files_and_never_opens_a_fifo_for_reading() {
    use crate::temp::Temp;
    use std::os::unix::ffi::OsStrExt;
    let temp = Temp::new("content-reader");
    let file = temp.path().join("bytes");
    std::fs::write(&file, vec![b'x'; CONTENT_HEAD + 100]).unwrap();
    let (_, data) = content_head(&file).unwrap();
    assert_eq!(data.len(), CONTENT_HEAD);
    let fifo = temp.path().join("pipe");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert!(content_head(&fifo).is_none());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn content_cache_reuses_reads_between_windows_sizes_and_theme_epochs() {
    use crate::temp::Temp;
    let temp = Temp::new("content-cache");
    let file = temp.path().join("image");
    std::fs::write(&file, b"image bytes").unwrap();
    let cache = Mutex::new(None);
    let reads = std::cell::Cell::new(0);
    for _ in 0..4 {
        let names = content_names_in(&cache, &file, true, |_| {
            reads.set(reads.get() + 1);
            "image/png".into()
        });
        assert_eq!(names, icon_names("image/png"));
    }
    assert_eq!(reads.get(), 1);
    std::fs::write(&file, b"a longer document").unwrap();
    let names = content_names_in(&cache, &file, true, |_| {
        reads.set(reads.get() + 1);
        "application/pdf".into()
    });
    assert_eq!(names, icon_names("application/pdf"));
    assert_eq!(reads.get(), 2);
    assert_eq!(cache.lock().unwrap().as_ref().unwrap().entries.len(), 1);
    assert_eq!(content_mime_with(b"\x89PNG", None), None);
}

#[test]
fn content_is_never_read_on_network_or_virtual_mounts() {
    use flit_platform_linux::mounts::parse;
    let mounts = parse(b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 0:20 / /proc rw - proc proc rw\n3 1 0:21 / /sys rw - sysfs sysfs rw\n4 1 0:30 / /mnt/share rw - cifs //server/share rw\n5 1 0:31 / /home/ann/remote rw - fuse.sshfs ann@host: rw\n6 1 0:32 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw\n7 1 0:33 / /tmp rw - tmpfs tmpfs rw\n8 1 0:34 / /mnt/nfs rw - nfs4 host:/x rw\n");
    for path in ["/proc/self/status", "/sys/kernel/notes", "/mnt/share/report", "/home/ann/remote/notes", "/run/user/1000/gvfs/smb-share/file", "/mnt/nfs/data"] {
        assert!(!content_readable(std::path::Path::new(path), &mounts), "{path}");
    }
    for path in ["/home/ann/notes", "/tmp/notes", "/mnt/sharex/file"] {
        assert!(content_readable(std::path::Path::new(path), &mounts), "{path}");
    }
    assert!(content_readable(std::path::Path::new("/anything"), &[]));
    let cache = Mutex::new(None);
    let names = content_names_in(&cache, std::path::Path::new("/proc/self/status"), false, |_| panic!("a virtual file was read"));
    assert!(names.is_empty());
}

#[test]
fn a_hung_content_read_does_not_hold_up_icons_of_other_files() {
    use crate::temp::Temp;
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let temp = Temp::new("content-hang");
    let slow = temp.path().join("slow");
    let fast = temp.path().join("fast");
    std::fs::write(&slow, b"slow bytes").unwrap();
    std::fs::write(&fast, b"fast bytes").unwrap();
    let cache = std::sync::Arc::new(Mutex::new(None));
    let (started, entered) = channel();
    let (release, hold) = channel::<()>();
    let hung = {
        let cache = cache.clone();
        std::thread::spawn(move || {
            content_names_in(&cache, &slow, true, |_| {
                started.send(()).unwrap();
                let _ = hold.recv_timeout(Duration::from_secs(10));
                "image/png".into()
            })
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done, finished) = channel();
    let other = {
        let cache = cache.clone();
        std::thread::spawn(move || done.send(content_names_in(&cache, &fast, true, |_| "text/plain".into())).unwrap())
    };
    let names = finished.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    other.join().unwrap();
    assert_eq!(hung.join().unwrap(), icon_names("image/png"));
    assert_eq!(names.expect("another file got its icon while the first read hung"), icon_names("text/plain"));
}
#[test]
fn content_type_results_are_freed_even_if_conversion_is_unknown() {
    use std::ffi::{CStr, CString};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FREED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn guess(name: *const libc::c_char, bytes: *const u8, size: usize, _: *mut libc::c_int) -> *mut libc::c_char {
        assert!(name.is_null());
        assert_eq!(unsafe { std::slice::from_raw_parts(bytes, size) }, b"image bytes");
        CString::new("image/png").unwrap().into_raw()
    }
    unsafe extern "C" fn mime(content: *const libc::c_char) -> *mut libc::c_char {
        assert_eq!(unsafe { CStr::from_ptr(content) }.to_bytes(), b"image/png");
        CString::new("image/png").unwrap().into_raw()
    }
    unsafe extern "C" fn unknown(_: *const libc::c_char) -> *mut libc::c_char {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn free(p: *mut libc::c_void) {
        if !p.is_null() {
            drop(unsafe { CString::from_raw(p.cast()) });
            FREED.fetch_add(1, Ordering::Relaxed);
        }
    }
    let api = ContentApi { guess, mime, free, _gio: 0, _glib: 0 };
    assert_eq!(content_mime_with(b"image bytes", Some(&api)).as_deref(), Some("image/png"));
    assert_eq!(FREED.load(Ordering::Relaxed), 2);
    let api = ContentApi { mime: unknown, ..api };
    assert_eq!(content_mime_with(b"image bytes", Some(&api)), None);
    assert_eq!(FREED.load(Ordering::Relaxed), 3);
}

#[test]
fn extensionless_png_and_replaced_content_choose_their_mime_icons_but_programs_do_not_sniff() {
    use crate::temp::Temp;
    use std::os::unix::fs::PermissionsExt;
    if content_api().is_none() {
        return;
    }
    let temp = Temp::new("content-icons");
    let path = temp.path().join("image");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0").unwrap();
    assert_eq!(content_icon_names(&path).first().map(String::as_str), Some("image-png"));
    std::fs::write(&path, b"%PDF-1.4\n").unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))).unwrap();
    assert_eq!(content_icon_names(&path).first().map(String::as_str), Some("application-pdf"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(content_icon_names(&path).is_empty(), "programs keep their program icon");
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(content_icon_names(&link).is_empty(), "the link pipeline retains its target and emblem handling");
}

#[test]
fn picker_can_identify_extensionless_text_and_keeps_missing_types_unknown() {
    use crate::temp::Temp;
    let temp = Temp::new("picker-mime");
    let readme = temp.path().join("readme");
    std::fs::write(&readme, b"hello world\n").unwrap();
    let mime = picker_mime(&readme).unwrap();
    assert!(mime.iter().any(|mime| mime == "text/plain"));
    assert!(mime.iter().any(|mime| mime == OCTET_STREAM));
    assert!(picker_mime(&temp.path().join("missing")).is_none());
}

#[test]
fn portal_can_filter_magic_only_types_and_normalizes_aliases() {
    let types = PickerTypes { registered: ["application/x-from-magic".into()].into_iter().collect(), aliases: [("application/x-magic-alias".into(), "application/x-from-magic".into())].into_iter().collect() };
    assert_eq!(types.resolve("application/x-from-magic".into()).as_deref(), Some("application/x-from-magic"));
    assert_eq!(types.resolve("application/x-magic-alias".into()).as_deref(), Some("application/x-from-magic"));
    assert_eq!(types.resolve("text/plain".into()).as_deref(), Some("text/plain"));
    assert_eq!(types.resolve("application/x-unknown".into()), None);
    assert_eq!(types.resolve("not a type".into()), None);
}
