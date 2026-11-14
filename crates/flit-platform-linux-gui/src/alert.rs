use std::ffi::OsStr;
use std::process::{Command, Stdio};

const TITLE: &str = "Flit";
const CLOSED: i32 = 1;

pub fn dialogs(message: &str) -> [Vec<String>; 3] {
    [
        vec!["zenity".into(), "--error".into(), "--no-markup".into(), format!("--title={TITLE}"), format!("--text={message}")],
        vec!["kdialog".into(), "--title".into(), TITLE.into(), "--error".into(), message.into()],
        vec!["notify-send".into(), "--urgency=critical".into(), format!("--app-name={TITLE}"), "--".into(), TITLE.into(), message.into()],
    ]
}

fn has_display() -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"].iter().any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

pub fn show(message: &str) -> bool {
    has_display()
        && dialogs(message).iter().any(|argv| {
            let Some(program) = crate::shell::on_path(OsStr::new(&argv[0])) else { return false };
            let status = Command::new(program).args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            status.is_ok_and(|s| s.code().is_some_and(|c| (0..=CLOSED).contains(&c)))
        })
}

pub fn notification(message: &str) -> bool {
    if !has_display() {
        return false;
    }
    let argv = ["notify-send".to_owned(), "--urgency=normal".to_owned(), format!("--app-name={TITLE}"), "--".to_owned(), TITLE.to_owned(), message.to_owned()];
    let Some(program) = crate::shell::on_path(OsStr::new(&argv[0])) else { return false };
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else { return false };
    let _ = std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
        let _ = child.wait();
    });
    true
}

#[cfg(test)]
#[path = "../test/alert.rs"]
mod tests;
