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

fn test_report(dir: PathBuf) -> Report {
    Report { dir, user: "TestUser".into(), home: r"C:\Users\TestUser".into(), os: "Test OS".into() }
}

#[test]
fn crash_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else { return };
    install_report(test_report(dir.into()));
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
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000);
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
fn a_crash_leaves_a_minidump_and_raw_facts_for_the_next_start() {
    let dir = crash_in_child("plain", false);
    let found = dumps(&dir);
    assert_eq!(found.len(), 1);
    let bytes = std::fs::read(&found[0]).unwrap();
    assert_eq!(&bytes[..4], b"MDMP");
    assert!(!found[0].with_extension("report.txt").exists());
    let raw = found[0].with_extension("raw");
    let facts = std::fs::read_to_string(&raw).unwrap();
    assert!(facts.contains("exception 0xC0000005") && !facts.contains("Recent log:"), "{facts}");
    assert!(facts.contains(".exe"), "the faulting module is named: {facts}");
    assert!(facts.contains("OS: Test OS"), "{facts}");
    #[cfg(target_arch = "x86_64")]
    assert!(facts.contains("#0"), "{facts}");
    std::fs::write(dir.join(flit_core::ui::LOG_FILE), "password=private-test-secret\n").unwrap();
    let modified = std::fs::metadata(&raw).unwrap().modified().unwrap();
    test_report(dir.clone()).finish();
    let report = found[0].with_extension("report.txt");
    let note = std::fs::read_to_string(&report).unwrap();
    assert!(note.contains("exception 0xC0000005") && note.contains("Recent log:\npassword=***"));
    assert!(!note.contains("private-test-secret") && !raw.exists());
    assert_eq!(std::fs::metadata(report).unwrap().modified().unwrap(), modified);
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
        std::fs::write(p.with_extension("raw"), b"x").unwrap();
        std::fs::write(p.with_extension("report.txt"), b"x").unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(100 - k);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
    }
    std::fs::write(dir.join("flit.log"), b"log").unwrap();
    prune(&dir, 3);
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, [
        "flit-crash-5.dmp", "flit-crash-5.raw", "flit-crash-5.report.txt", "flit-crash-5.txt",
        "flit-crash-6.dmp", "flit-crash-6.raw", "flit-crash-6.report.txt", "flit-crash-6.txt",
        "flit-crash-7.dmp", "flit-crash-7.raw", "flit-crash-7.report.txt", "flit-crash-7.txt",
        "flit.log",
    ]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_shareable_report_includes_only_the_latest_fifty_log_lines() {
    let dir = temp("recent-log");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("flit.1.log"), (0..60).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
    std::fs::write(dir.join(flit_core::ui::LOG_FILE), "password=private\nmail user@example.org\n").unwrap();
    let report = test_report(dir.clone());
    let text = report.text(r"0: module at C:\Users\TestUser\flit.exe");
    assert!(text.contains(r"0: module at ~\flit.exe"));
    let log = text.split_once("Recent log:\n").unwrap().1;
    assert_eq!(log.lines().count(), 50);
    assert_eq!(log.lines().next(), Some("line 12"));
    assert!(log.ends_with("password=***\nmail \n"));
    assert!(!text.contains("TestUser") && !text.contains("private") && !text.contains("example.org"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flit_asks_windows_to_restart_it_after_updates_but_not_after_a_crash() {
    use windows::Win32::System::Recovery::GetApplicationRestartSettings;
    register_restart("--restarted-by-windows");
    let (mut line, mut len, mut flags) = ([0u16; 64], 64u32, 0u32);
    unsafe { GetApplicationRestartSettings(GetCurrentProcess(), Some(windows::core::PWSTR(line.as_mut_ptr())), &mut len, Some(&mut flags)) }.unwrap();
    assert_eq!(String::from_utf16_lossy(&line[..len as usize - 1]), "--restarted-by-windows");
    assert_eq!(flags, (RESTART_NO_CRASH | RESTART_NO_HANG).0);
}
