use super::*;

#[test]
#[cfg(target_os = "linux")]
fn the_sandbox_only_mounts_the_input_and_runtime_read_only() {
    let cmd = sandbox(Path::new("/usr/bin/bwrap"), Path::new("/home/user/flit"), Path::new("/home/user/sample.rar"), Path::new("/source/sample.rar"));
    let args: Vec<_> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();
    assert!(args.contains(&"--unshare-all") && args.contains(&"--clearenv") && args.contains(&"--die-with-parent"));
    assert!(args.windows(3).any(|a| a == ["--ro-bind", "/home/user/sample.rar", "/source/sample.rar"]));
    assert!(!args.windows(3).any(|a| a[0] == "--ro-bind" && matches!(a[1], "/" | "/home" | "/home/user")));
    assert!(!args.iter().any(|a| matches!(*a, "--share-net" | "--bind" | "--proc")));
}

#[test]
#[cfg(windows)]
fn the_parser_discards_failed_truncated_and_cancelled_commands() {
    let command = |script: &str| {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/D", "/C", script]);
        cmd
    };
    let run = |script, cap| collect(&mut command(script), cap, &|| true, TIMEOUT);
    assert_eq!(run("echo hello", 32).unwrap(), b"hello\r\n");
    assert_eq!(run("echo hello", 4), Err(Error::Failed));
    assert_eq!(run("echo hello & exit /B 12", 32), Err(Error::Failed));
    assert_eq!(run("exit /B 3", 32), Err(Error::TooLarge));
    assert_eq!(run("exit /B 4", 32), Err(Error::PasswordRequired));
    assert_eq!(run("exit /B 5", 32), Err(Error::Unsupported));
    assert_eq!(run("echo hello & exit /B 3", 32), Err(Error::Failed));
    assert_eq!(collect(&mut command("echo hello"), 32, &|| false, TIMEOUT), Err(Error::Cancelled));
    assert_eq!(collect(&mut command("echo hello"), 32, &|| true, Duration::ZERO), Err(Error::TimedOut));
}

#[test]
#[cfg(target_os = "linux")]
fn bwrap_is_probed_once_with_the_helper_and_the_runtime_mounts() {
    use std::os::unix::fs::PermissionsExt;

    let exe = std::env::current_exe().unwrap();
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = std::env::temp_dir().join(format!("flit-bwrap-test-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let fixture = Fixture(dir);
    let wrapper = fixture.0.join("bwrap");
    let input = fixture.0.join("sample.zip");
    std::fs::write(&input, []).unwrap();

    for (exit_code, available) in [(1, false), (0, true)] {
        let script = format!("#!/bin/sh\ncase \" $* \" in\n  *\" --ro-bind \"*\" /flit --ro-bind \"*\" /source/flit --remount-ro / -- /flit --parse-preview probe /source/flit \") exit {exit_code};;\n  *) exit 1;;\nesac\n");
        std::fs::write(&wrapper, script).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cache = OnceLock::new();
        let first = sandbox_command(&cache, &exe, &input, || Some(wrapper.clone()));
        assert_eq!(first.is_some(), available);
        std::fs::remove_file(&wrapper).unwrap();
        let again = sandbox_command(&cache, &exe, &input, || panic!("bwrap was probed again"));
        assert_eq!(again.is_some(), available);
        if let Some((cmd, mapped)) = again {
            assert_eq!(cmd.get_program(), wrapper.as_os_str());
            assert_eq!(mapped, Path::new("/source/sample.zip"));
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn an_unlinked_executable_is_launched_from_proc() {
    let cmd = command(Path::new("/flit (deleted)"), Path::new("/source.zip"), "archive", &[]);
    assert_eq!(cmd.get_program(), Path::new("/proc/self/exe").as_os_str());
}

#[test]
#[cfg(target_os = "linux")]
fn killing_or_timing_out_a_parser_has_distinct_results() {
    let command = |script| {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    };
    assert_eq!(collect(&mut command("kill -TERM $$"), 0, &|| true, TIMEOUT), Err(Error::Failed));
    assert_eq!(collect(&mut command("exec sleep 30"), 0, &|| true, Duration::from_millis(50)), Err(Error::TimedOut));
}
