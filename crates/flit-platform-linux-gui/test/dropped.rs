use super::*;
use crate::temp::Temp;

fn save(dir: &Path, content: &Content, text_stem: &str, image_stem: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let file = content.clone().file(text_stem, image_stem).unwrap();
    let out = queued(&queued_path(dir), &file, &Job::detached(flit_core::provider::Resolve::KeepBoth));
    if let Some(error) = out.error {
        return Err(std::io::Error::other(error.message));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(out.items[0].1.as_bytes().to_vec())))
}

#[test]
fn only_web_addresses_count_as_dropped_links() {
    let list = b"# from a browser\r\nhttps://example.org/a%20b/page.html?x=1\r\nfile:///tmp/a\r\nHTTP://Example.org\r\nmailto:x@y\r\nhttp://\r\n";
    assert_eq!(web_urls(list), ["https://example.org/a%20b/page.html?x=1", "HTTP://Example.org"]);
    assert!(web_urls(b"file:///home/u/x.txt\r\n").is_empty(), "files are dropped as files");
}

#[test]
fn a_dropped_link_is_named_after_its_site_and_page() {
    assert_eq!(url_stem("https://example.org/docs/a%20b.html?x=1#top").as_deref(), Some("example.org a b.html"));
    assert_eq!(url_stem("https://user@example.org/").as_deref(), Some("example.org"));
    assert_eq!(url_stem("http://example.org/a/b/").as_deref(), Some("example.org b"));
    assert_eq!(url_stem("https://x.org/a%2F..%2Fb").as_deref(), Some("x.org a .. b"), "slashes never reach the file name");
}

#[test]
fn dropped_links_text_and_pictures_become_new_files_without_replacing_any() {
    let t = Temp::new("dropped");
    let dir = t.path();
    let link = save(dir, &Content::Url("https://example.org/page".into()), "Dropped text", "Dropped image").unwrap();
    assert_eq!(link, dir.join("example.org page.desktop"));
    let body = std::fs::read_to_string(&link).unwrap();
    assert!(body.starts_with("[Desktop Entry]\nType=Link\n") && body.contains("\nURL=https://example.org/page\n"), "{body}");
    let first = save(dir, &Content::Text("hello".into()), "Dropped text", "Dropped image").unwrap();
    let second = save(dir, &Content::Text("again".into()), "Dropped text", "Dropped image").unwrap();
    assert_eq!((first.file_name().unwrap(), second.file_name().unwrap()), ("Dropped text.txt".as_ref(), "Dropped text (2).txt".as_ref()));
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "hello", "the first file is not overwritten");
    let png = save(dir, &Content::Image(vec![0x89, b'P', b'N', b'G']), "Dropped text", "Dropped image").unwrap();
    assert_eq!(png, dir.join("Dropped image.png"));
    assert_eq!(std::fs::read(&png).unwrap(), [0x89, b'P', b'N', b'G']);
}

#[test]
fn a_link_with_a_newline_cannot_add_keys_to_the_launcher() {
    let body = link_file("https://x.org/\nExec=rm -rf ~", "x\nExec=evil");
    assert_eq!(body.lines().filter(|l| l.starts_with("Exec=")).count(), 0, "{body}");
}

#[test]
fn files_win_over_links_pictures_and_text_offered_with_them() {
    let browser = [TypeHint::Plaintext, PNG, TypeHint::Html, TypeHint::UriList];
    assert_eq!(wanted(&browser), [TypeHint::UriList, PNG, TypeHint::Plaintext]);
    assert_eq!(wanted(&[TypeHint::Html]), Vec::<TypeHint>::new(), "only HTML is not taken");
    assert_eq!(wanted(&[]), [TypeHint::UriList], "unknown types are tried as a file list");
    let p = |s: &str| FsPath::parse(s).unwrap();
    assert_eq!(interpret(Some(TypeHint::UriList), b"file:///tmp/a\r\nhttps://x.org/\r\n"), Some(Dragged::Files(vec![p("/tmp/a")])));
    assert_eq!(interpret(Some(TypeHint::UriList), b"https://x.org/\r\n"), Some(Dragged::Content(Content::Url("https://x.org/".into()))));
    assert_eq!(interpret(Some(TypeHint::UriList), b"mailto:a@b\r\n"), None, "the next offered type is tried");
}

#[test]
fn dropped_text_that_is_only_an_address_becomes_a_link_and_other_text_stays_text() {
    assert_eq!(interpret(Some(TypeHint::Plaintext), b" https://x.org/a \n"), Some(Dragged::Content(Content::Url("https://x.org/a".into()))));
    assert_eq!(interpret(Some(TypeHint::Plaintext), b"see https://x.org/a"), Some(Dragged::Content(Content::Text("see https://x.org/a".into()))));
    assert_eq!(interpret(Some(TypeHint::Plaintext), b"  \n"), None, "blank text makes no file");
    assert_eq!(interpret(Some(PNG), b"\x89PNG"), Some(Dragged::Content(Content::Image(b"\x89PNG".to_vec()))));
    assert_eq!(interpret(Some(PNG), b""), None);
}

#[test]
fn repository_drops_reuse_the_link_data_and_keep_ordinary_browser_links() {
    for (hint, bytes, expected) in [
        (TypeHint::UriList, b"https://github.com/owner/repo?tab=readme\r\n".as_slice(), Some("https://github.com/owner/repo")),
        (TypeHint::Plaintext, b"git@host:owner/repo".as_slice(), Some("git@host:owner/repo")),
        (TypeHint::UriList, b"https://github.com/owner/repo/issues\r\n".as_slice(), None),
    ] {
        let Some(Dragged::Content(content)) = interpret(Some(hint), bytes) else { panic!("browser content") };
        assert_eq!(content.repository_url(), expected);
    }
}

#[test]
fn repository_only_uri_drops_have_no_link_file_fallback() {
    for url in ["ssh://git@host/owner/repo.git", "git@host:owner/repo.git"] {
        let bytes = format!("# Browser drop\r\n{url}\r\n");
        let Some(Dragged::Content(content)) = interpret(Some(TypeHint::UriList), bytes.as_bytes()) else { panic!("repository address") };
        assert_eq!(content.repository_url(), Some(url));
        assert!(content.file("Dropped text", "Dropped image").is_none());
    }
    let Some(Dragged::Content(content)) = interpret(Some(TypeHint::UriList), b"https://github.com/owner/repo\r\n") else { panic!("web address") };
    assert!(content.file("Dropped text", "Dropped image").unwrap().name.ends_with(b".desktop"));
}

fn queued_file(name: &str, bytes: &[u8]) -> flit_core::provider::DroppedFile {
    flit_core::provider::DroppedFile { name: name.as_bytes().to_vec(), bytes: bytes.to_vec() }
}

fn queued_path(path: &Path) -> FsPath {
    use std::os::unix::ffi::OsStrExt;
    FsPath::parse_exact(path.as_os_str().as_bytes()).unwrap()
}

#[test]
fn queued_links_text_and_png_keep_their_names_bytes_and_creation_reports() {
    use flit_core::provider::{FileOp, Resolve};
    use flit_platform::{FileOps, Job};
    let t = Temp::new("queued-drop");
    let dest = queued_path(t.path());
    let cases = [
        (Content::Url("https://example.org/page".into()), "example.org page.desktop", b"[Desktop Entry]\nType=Link\nName=example.org page\nURL=https://example.org/page\nIcon=text-html\n".as_slice()),
        (Content::Text("hello\nworld".into()), "Dropped text.txt", b"hello\nworld"),
        (Content::Image(b"\x89PNG\r\n\x1a\n".to_vec()), "Dropped image.png", b"\x89PNG\r\n\x1a\n"),
    ];
    for (content, name, bytes) in cases {
        let data = std::sync::Arc::new(content.file("Dropped text", "Dropped image").unwrap());
        let job = Job::detached(Resolve::Skip);
        let out = crate::shell::LinuxShell.file_op(&FileOp::PasteDropped { dest: dest.clone(), moving: false, data: Some(data) }, &job);
        assert!(out.error.is_none(), "{:?}", out.error);
        let target = dest.join(name.as_bytes());
        assert_eq!(out.items, [(target.clone(), target)]);
        assert_eq!(std::fs::read(t.path().join(name)).unwrap(), bytes);
        assert_eq!(job.progress().items, 1);
    }
}

#[test]
fn queued_drops_in_one_folder_keep_their_own_payload_when_workers_run_in_reverse() {
    use flit_core::provider::{FileOp, Resolve};
    use flit_platform::{FileOps, Job};
    let t = Temp::new("queued-order");
    let dest = queued_path(t.path());
    let first = FileOp::PasteDropped { dest: dest.clone(), moving: false, data: Some(std::sync::Arc::new(queued_file("first.txt", b"first payload"))) };
    let second = FileOp::PasteDropped { dest: dest.clone(), moving: false, data: Some(std::sync::Arc::new(queued_file("second.txt", b"second payload"))) };
    let second_out = crate::shell::LinuxShell.file_op(&second, &Job::detached(Resolve::Skip));
    let first_out = crate::shell::LinuxShell.file_op(&first, &Job::detached(Resolve::Skip));
    assert!(first_out.error.is_none() && second_out.error.is_none());
    assert_eq!(first_out.items, [(dest.join(b"first.txt"), dest.join(b"first.txt"))]);
    assert_eq!(second_out.items, [(dest.join(b"second.txt"), dest.join(b"second.txt"))]);
    assert_eq!(std::fs::read(t.path().join("first.txt")).unwrap(), b"first payload");
    assert_eq!(std::fs::read(t.path().join("second.txt")).unwrap(), b"second payload");
}

#[test]
fn cancelling_one_queued_drop_before_start_preserves_the_other_drop() {
    use flit_core::provider::{FileOp, Resolve};
    use flit_platform::{FileOps, Job};
    let t = Temp::new("queued-cancel");
    let dest = queued_path(t.path());
    let first = FileOp::PasteDropped { dest: dest.clone(), moving: false, data: Some(std::sync::Arc::new(queued_file("same.txt", b"cancelled"))) };
    let second = FileOp::PasteDropped { dest: dest.clone(), moving: false, data: Some(std::sync::Arc::new(queued_file("same.txt", b"kept"))) };
    let cancelled = Job::detached(Resolve::Replace);
    cancelled.ctl().cancel();
    let out = crate::shell::LinuxShell.file_op(&first, &cancelled);
    assert!(out.items.is_empty());
    assert!(out.error.is_some());
    assert!(!t.path().join("same.txt").exists());
    let kept = crate::shell::LinuxShell.file_op(&second, &Job::detached(Resolve::Skip));
    assert!(kept.error.is_none(), "{:?}", kept.error);
    assert_eq!(kept.items, [(dest.join(b"same.txt"), dest.join(b"same.txt"))]);
    assert_eq!(std::fs::read(t.path().join("same.txt")).unwrap(), b"kept");
}

#[test]
fn queued_drops_use_copy_conflict_choices_without_overwriting_on_skip_or_keep_both() {
    use flit_core::provider::Resolve;
    use flit_platform::Job;
    for (choice, expected) in [(Resolve::Skip, "same.txt"), (Resolve::KeepBoth, "same (2).txt"), (Resolve::Replace, "same.txt")] {
        let t = Temp::new("queued-conflict");
        std::fs::write(t.path().join("same.txt"), b"original").unwrap();
        let dest = queued_path(t.path());
        let out = queued(&dest, &queued_file("same.txt", b"dropped"), &Job::detached(choice));
        assert!(out.error.is_none(), "{:?}", out.error);
        if choice == Resolve::Skip {
            assert!(out.items.is_empty());
        } else {
            let target = dest.join(expected.as_bytes());
            assert_eq!(out.items, [(target.clone(), target)]);
            assert_eq!(std::fs::read(t.path().join(expected)).unwrap(), b"dropped");
        }
        assert_eq!(std::fs::read(t.path().join("same.txt")).unwrap(), if choice == Resolve::Replace { b"dropped".as_slice() } else { b"original" });
    }
}

#[test]
fn a_staged_drop_is_private_and_removed_when_it_is_released() {
    use std::os::unix::fs::PermissionsExt;
    let staged = Staged::new(&queued_file("text.txt", b"hello"), &flit_platform::Job::detached(flit_core::provider::Resolve::Skip)).unwrap();
    let path = staged.path.clone();
    let parent = path.parent().unwrap().to_owned();
    assert_eq!(std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    drop(staged);
    assert!(!parent.exists());
}

#[test]
fn queued_drop_names_cannot_escape_the_destination_or_touch_existing_files() {
    use flit_core::provider::Resolve;
    use flit_platform::Job;
    let t = Temp::new("queued-name");
    let dest = queued_path(t.path());
    for name in ["../escape", "/absolute", "", ".", "..", "a/b", "bad\0name", "name\nline"] {
        let out = queued(&dest, &queued_file(name, b"bad"), &Job::detached(Resolve::Replace));
        assert!(out.error.is_some(), "{name:?}");
        assert!(out.items.is_empty());
    }
    assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
}

#[test]
fn queued_drops_report_readonly_destination_errors_even_when_the_suite_runs_as_root() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let t = Temp::new("queued-readonly");
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", "dropped::tests::queued_drop_readonly_child", "--ignored", "--nocapture"]).env("FLIT_DROP_READONLY_TEST_DIR", t.path());
    if unsafe { libc::geteuid() } == 0 {
        child.uid(65534).gid(65534);
    }
    let result = child.output().unwrap();
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.status.success(), "{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
}

#[test]
#[ignore]
fn queued_drop_readonly_child() {
    use flit_core::provider::Resolve;
    use flit_platform::Job;
    let Some(dir) = std::env::var_os("FLIT_DROP_READONLY_TEST_DIR") else { return };
    let out = queued(&queued_path(Path::new(&dir)), &queued_file("text.txt", b"hello"), &Job::detached(Resolve::Skip));
    assert!(out.items.is_empty());
    assert!(std::fs::read_dir(&dir).is_ok(), "the child must reach the folder so only its read-only mode can refuse the drop");
    let error = out.error.unwrap();
    assert!(error.message.contains("Permission denied"), "{}", error.message);
}

#[test]
fn cancelling_while_the_drop_is_staged_removes_its_private_files() {
    use flit_core::provider::{OpCtl, Stage};
    use std::sync::{Arc, Mutex};
    let t = Temp::new("cancel-stage");
    let dest = queued_path(t.path());
    let ctl = OpCtl::default();
    let cancelled = ctl.clone();
    let found = Arc::new(Mutex::new(None));
    let seen = found.clone();
    let prefix = format!("flit-drop-{}-", std::process::id());
    let job = Job::new(
        ctl,
        Box::new(move |p| {
            if p.stage == Stage::Preparing {
                let stage = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(Result::ok).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&prefix) && p.join("cancel-writing.txt").exists());
                *seen.lock().unwrap() = stage;
                cancelled.cancel();
            }
        }),
        Box::new(|_| panic!("no conflict before staging finishes")),
    );
    let out = queued(&dest, &queued_file("cancel-writing.txt", &vec![b'x'; WRITE_CHUNK * 2]), &job);
    assert!(out.items.is_empty());
    assert_eq!(out.error.unwrap().message, flit_platform::CANCELLED);
    assert!(!found.lock().unwrap().as_ref().unwrap().exists());
    assert!(!t.path().join("cancel-writing.txt").exists());
}

#[test]
fn unrelated_preexisting_staging_directories_are_never_reused_or_removed() {
    let t = Temp::new("foreign-stage");
    let foreign = t.path().join(format!("flit-drop-{}-0-0", std::process::id()));
    std::fs::create_dir(&foreign).unwrap();
    let marker = foreign.join("foreign-marker");
    std::fs::write(&marker, b"leave this alone").unwrap();
    let staged = Staged::new_at(t.path(), &queued_file("text.txt", b"new"), &Job::detached(flit_core::provider::Resolve::Skip), 0).unwrap();
    assert_ne!(staged.dir, foreign);
    drop(staged);
    assert_eq!(std::fs::read(&marker).unwrap(), b"leave this alone");
}
#[test]
fn malformed_non_ascii_uri_lines_do_not_panic_or_become_files() {
    assert!(web_urls("🤔🤔🤔".as_bytes()).is_empty());
    assert_eq!(interpret(Some(TypeHint::UriList), "🤔🤔🤔".as_bytes()), None);
}

#[test]
fn queued_files_keep_the_same_default_permissions_as_ordinary_new_files() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new("queued-mode");
    let ordinary = t.path().join("ordinary.txt");
    std::fs::File::create_new(&ordinary).unwrap();
    let out = queued(&queued_path(t.path()), &queued_file("dropped.txt", b"hello"), &Job::detached(flit_core::provider::Resolve::Skip));
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(std::fs::metadata(t.path().join("dropped.txt")).unwrap().permissions().mode() & 0o777, std::fs::metadata(ordinary).unwrap().permissions().mode() & 0o777);
}
