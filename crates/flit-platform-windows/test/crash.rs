use super::*;
use std::process::Command;

const CHILD_ENV: &str = "FLIT_TEST_CRASH";

fn temp(tag: &str) -> PathBuf {
    let d = crate::test_temp().join(format!("flit-crash-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

unsafe extern "system" fn foreign(_: *const EXCEPTION_POINTERS) -> i32 {
    0
}

#[test]
fn crash_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else { return };
    install(Path::new(&dir));
    if std::env::var_os("FLIT_TEST_CRASH_FOREIGN").is_some() {
        unsafe {
            SetUnhandledExceptionFilter(Some(foreign));
        }
        rearm();
    }
    unsafe {
        std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1);
    }
}

fn crash_in_child(tag: &str, foreign_filter: bool) -> PathBuf {
    let dir = temp(tag);
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "crash::tests::crash_child", "--nocapture", "--test-threads=1"]).env(CHILD_ENV, &dir);
    if foreign_filter {
        cmd.env("FLIT_TEST_CRASH_FOREIGN", "1");
    }
    let status = cmd.output().unwrap().status;
    assert_eq!(status.code().map(|c| c as u32), Some(0xC000_0005), "the child dies of the access violation");
    dir
}

fn dumps(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "dmp")).collect()
}

#[test]
fn a_crash_leaves_a_minidump_and_a_note() {
    let dir = crash_in_child("plain", false);
    let found = dumps(&dir);
    assert_eq!(found.len(), 1);
    let bytes = std::fs::read(&found[0]).unwrap();
    assert_eq!(&bytes[..4], b"MDMP");
    let note = std::fs::read_to_string(found[0].with_extension("txt")).unwrap();
    assert!(note.contains("exception 0xC0000005"), "{note}");
    assert!(note.contains(".exe"), "the faulting module is named: {note}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_filter_replaced_by_a_shell_extension_is_rearmed() {
    let dir = crash_in_child("foreign", true);
    assert_eq!(dumps(&dir).len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn prune_keeps_the_newest_dumps() {
    let dir = temp("prune");
    std::fs::create_dir_all(&dir).unwrap();
    for k in 0..8 {
        let p = dir.join(format!("{PREFIX}{k}.dmp"));
        std::fs::write(&p, b"MDMP").unwrap();
        std::fs::write(p.with_extension("txt"), b"x").unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(100 - k);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
    }
    std::fs::write(dir.join("flit.log"), b"log").unwrap();
    prune(&dir, 3);
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["flit-crash-5.dmp", "flit-crash-5.txt", "flit-crash-6.dmp", "flit-crash-6.txt", "flit-crash-7.dmp", "flit-crash-7.txt", "flit.log"]);
    let _ = std::fs::remove_dir_all(dir);
}
