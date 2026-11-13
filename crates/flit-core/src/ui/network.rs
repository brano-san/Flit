use super::palette::{Palette, Pick};
use super::App;
use crate::i18n::S;
use crate::open::Open;
use crate::path::PathKind;
use crate::provider::Launch;

const SCHEMES: [&str; 9] = ["smb", "sftp", "ssh", "ftp", "ftps", "dav", "davs", "afp", "nfs"];

pub fn server_uri(text: &str) -> Option<String> {
    let text = text.trim();
    let (scheme, rest) = text.split_once("://")?;
    let host = rest.split('/').next()?.rsplit('@').next()?;
    let clean = !text.chars().any(|c| c.is_whitespace() || c.is_control());
    (SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) && !host.is_empty() && clean).then(|| text.to_owned())
}

pub(super) fn native_url(text: &str) -> bool {
    let scheme = text.split_once(':').map_or("", |(s, _)| s);
    ["file", "trash", "recent"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) || server_uri(text).is_some()
}

pub(super) fn split_remote(cmd: &Open) -> Option<(Vec<String>, Open)> {
    let uri = |p: &Vec<u8>| std::str::from_utf8(p).ok().and_then(server_uri);
    let recent = |p: &Vec<u8>| p.len() >= 7 && p[..7].eq_ignore_ascii_case(b"recent:");
    let remote = |p: &Vec<u8>| uri(p).is_some() || recent(p);
    if !cmd.paths.iter().chain(&cmd.select).any(remote) {
        return None;
    }
    let servers = cmd.paths.iter().chain(&cmd.select).filter_map(uri).collect();
    let rest = Open { paths: cmd.paths.iter().filter(|p| !remote(p)).cloned().collect(), select: cmd.select.iter().filter(|p| !remote(p)).cloned().collect(), ..cmd.clone() };
    Some((servers, rest))
}

impl App {
    pub(super) fn map_drive(&mut self) {
        let share = self.tab().view_loc.to_windows().filter(|p| p.kind() == PathKind::Unc);
        self.launch(Launch::MapDrive(share));
    }

    pub(super) fn ask_server(&mut self) {
        self.open_pick(Palette::new(Pick::Server, Vec::new(), ""));
    }

    pub(super) fn connect_server(&mut self, text: &str) {
        match server_uri(text) {
            Some(uri) => self.launch(Launch::ConnectServer(uri)),
            None => self.status(S::SidebarDriveServerPrompt.t()),
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/network.rs"]
mod tests;
