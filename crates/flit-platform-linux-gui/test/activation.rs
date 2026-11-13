use super::*;
use std::collections::HashMap;
use std::ffi::OsStr;

fn envs(cmd: &Command) -> HashMap<&OsStr, Option<&OsStr>> {
    cmd.get_envs().collect()
}

#[test]
fn spawned_programs_get_the_token_or_lose_an_inherited_one() {
    let mut given = Command::new("true");
    pass(&mut given, Some("tok_TIME5"));
    let e = envs(&given);
    assert_eq!(e[OsStr::new("XDG_ACTIVATION_TOKEN")], Some(OsStr::new("tok_TIME5")));
    assert_eq!(e[OsStr::new("DESKTOP_STARTUP_ID")], Some(OsStr::new("tok_TIME5")));
    let mut none = Command::new("true");
    pass(&mut none, None);
    let e = envs(&none);
    assert_eq!((e[OsStr::new("XDG_ACTIVATION_TOKEN")], e[OsStr::new("DESKTOP_STARTUP_ID")]), (None, None));
    let out = Command::new("sh").args(["-c", "echo \"$XDG_ACTIVATION_TOKEN|$DESKTOP_STARTUP_ID\""]).env("XDG_ACTIVATION_TOKEN", "old").env("DESKTOP_STARTUP_ID", "old").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "old|old");
    let mut cleared = Command::new("sh");
    cleared.args(["-c", "echo \"$XDG_ACTIVATION_TOKEN|$DESKTOP_STARTUP_ID\""]).env("XDG_ACTIVATION_TOKEN", "old").env("DESKTOP_STARTUP_ID", "old");
    pass(&mut cleared, None);
    assert_eq!(String::from_utf8_lossy(&cleared.output().unwrap().stdout).trim(), "|");
}

#[test]
fn the_launcher_token_is_read_from_the_variable_of_the_session() {
    let vars = |w: Option<&str>, x: Option<&str>| {
        let (w, x) = (w.map(str::to_owned), x.map(str::to_owned));
        move |k: &str| if k == "XDG_ACTIVATION_TOKEN" { w.clone() } else { x.clone() }
    };
    assert_eq!(from_env(vars(Some("wl"), Some("x11")), true).as_deref(), Some("wl"));
    assert_eq!(from_env(vars(Some("wl"), Some("x11")), false).as_deref(), Some("x11"));
    assert_eq!(from_env(vars(None, Some("x11")), true).as_deref(), Some("x11"));
    assert_eq!(from_env(vars(Some(" "), None), true), None);
}

#[test]
fn a_launch_waits_briefly_for_a_requested_token() {
    asked();
    let answer = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(40));
        answered("fresh".into());
    });
    let started = Instant::now();
    assert_eq!(take_waiting(Duration::from_secs(5)).as_deref(), Some("fresh"));
    assert!(started.elapsed() >= Duration::from_millis(30));
    answer.join().unwrap();
    assert_eq!(take_waiting(Duration::from_secs(5)), None, "a token is used once");
    asked();
    let started = Instant::now();
    assert_eq!(take_waiting(Duration::from_millis(50)), None);
    assert!(started.elapsed() < Duration::from_secs(1));
    let started = Instant::now();
    assert_eq!(take_waiting(Duration::from_secs(5)), None, "an unanswered request is not waited for twice");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn x11_startup_ids_carry_the_server_time_and_quote_the_remove_message() {
    let id = startup_id("box", 7, 2, 123456);
    assert_eq!(id, "box-flit-7-2_TIME123456");
    assert_eq!(startup_time(&id), Some(123456));
    assert_eq!(startup_time("kwin-launch"), None);
    assert_eq!(remove_message("a\"b\\c"), b"remove: ID=\"a\\\"b\\\\c\"\0");
}

#[test]
fn an_x_server_gives_a_startup_id_with_its_time() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no X server, skipped");
        return;
    }
    let id = x11_token().unwrap();
    assert!(startup_time(&id).is_some_and(|t| t > 0), "{id}");
}
