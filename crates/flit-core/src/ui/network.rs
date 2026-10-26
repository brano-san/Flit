use super::palette::{Palette, Pick};
use super::App;
use crate::i18n::S;
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
