use super::*;

#[test]
fn the_inhibitor_blocks_sleep_until_its_stdin_closes() {
    let a: Vec<String> = args("Copying files").into_iter().map(|s| s.into_string().unwrap()).collect();
    assert_eq!(a, ["--what=sleep", "--who=Flit", "--why=Copying files", "--mode=block", "cat"]);
}

#[test]
fn dropping_the_guard_ends_the_holder_process() {
    let child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
    let pid = child.id() as i32;
    drop(Awake(child));
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "the holder is gone once the guard is dropped");
}
