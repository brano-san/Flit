use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

const INHIBITORS: [&str; 2] = ["systemd-inhibit", "elogind-inhibit"];

pub struct Awake(Child);

pub fn args(reason: &str) -> Vec<OsString> {
    ["--what=sleep", "--who=Flit", &format!("--why={reason}"), "--mode=block", "cat"].into_iter().map(OsString::from).collect()
}

impl Awake {
    pub fn hold(reason: &str) -> Option<Self> {
        let program = INHIBITORS.iter().find_map(|p| crate::shell::on_path(OsStr::new(p)))?;
        let mut cmd = Command::new(&program);
        cmd.args(args(reason)).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
        cmd.spawn().map(Awake).map_err(|e| tracing::warn!("cannot keep the system awake with {}: {e}", program.display())).ok()
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
#[path = "../test/awake.rs"]
mod tests;
