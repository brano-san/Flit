use super::archive::Kind;
use super::form::{Control, Form};
use super::menu::Menu;
use super::sidebar::{Drive, PlaceCmd};
use super::{App, Effect};
use crate::archive::Secret;
use crate::i18n::S;
use crate::location::ProviderId;
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, ProviderRequest, TabId};
use crate::name;

const ENCRYPTED: [&str; 2] = ["crypto_LUKS", "BitLocker"];
const SYSTEM_MOUNTS: [&[u8]; 2] = [b"/", b"/home"];

impl Drive {
    pub fn locked(&self) -> bool {
        self.mountable() && ENCRYPTED.contains(&self.file_system.as_str())
    }

    pub fn mountable(&self) -> bool {
        self.unmounted && self.remote.is_none() && self.path.kind() == PathKind::Unix
    }

    pub fn unmountable(&self) -> bool {
        !self.unmounted && self.remote.is_none() && self.path.kind() == PathKind::Unix && !SYSTEM_MOUNTS.contains(&self.path.as_bytes())
    }
}

impl App {
    fn drive_at(&self, path: &FsPath) -> Option<&Drive> {
        self.nav.sidebar.places.drives.iter().find(|d| d.path == *path)
    }

    pub(super) fn mount_drive(&mut self, path: &FsPath) -> bool {
        let drive = self.drive_at(path);
        if !drive.is_some_and(|d| d.unmounted) && !(path.kind() == PathKind::Unix && self.nav.sidebar.places.offline_path(path)) {
            return false;
        }
        if let Some(d) = drive.filter(|d| d.locked()) {
            let (device, title) = (d.path.clone(), d.title());
            self.ask_unlock(device, &title, false);
            return true;
        }
        let (tab, launch) = (self.tab().id, Launch::Mount(path.clone()));
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
        true
    }

    pub(super) fn drive_items(&self, m: &mut Menu, path: &FsPath) {
        let Some(d) = self.drive_at(path) else { return };
        if d.unmountable() {
            m.place(S::MenuUnmount.t(), PlaceCmd::Launch(Launch::Unmount(d.path.clone())), false);
        }
        if !d.unmounted && d.remote.is_some() && d.path.kind() == PathKind::Unix {
            m.place(S::SidebarDriveDisconnect.t(), PlaceCmd::Launch(Launch::Disconnect(Some(d.path.clone()))), false);
        }
        if d.offline() {
            m.place(S::SidebarDriveLoginConnect.t(), PlaceCmd::Mount(path.clone()), false);
        } else if d.mountable() {
            m.place(if d.locked() { S::SidebarDriveUnlock.t() } else { S::MenuMount.t() }, PlaceCmd::Mount(d.path.clone()), false);
        }
    }

    pub(super) fn computer_drive_items(&self, m: &mut Menu) {
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|v| t.view_loc.path().kind() == PathKind::Computer && v.selected_count() == 1) else { return };
        let Some(i) = v.selected().next() else { return };
        let Some(location) = t.entry_location(i) else { return };
        let path = location.path().clone();
        if self.drive_at(&path).is_some_and(|d| d.removable && !d.unmounted && d.path.kind() == PathKind::Unix) {
            m.place(S::MenuEject.t(), PlaceCmd::Eject(path.clone()), false);
        }
        self.drive_items(m, &path);
    }

    pub(super) fn ask_unlock(&mut self, device: FsPath, title: &str, wrong: bool) -> bool {
        if self.dialogs.form.is_some() {
            return false;
        }
        let controls = vec![Control::value(S::SidebarDriveVolume.t(), title), Control::secret(S::SidebarDrivePassphrase.t(), "")];
        let mut f = Form::new(Kind::Unlock { device }, S::SidebarDriveUnlockTitle.t(), controls, vec![S::SidebarDriveUnlock.t(), S::SettingsPageDialogCancel.t()]);
        if wrong {
            f.say(S::ErrorsFsWrongPassphrase.t(), true);
        }
        self.open_form(f);
        true
    }

    pub(super) fn unlock_again(&mut self, tab: TabId, device: FsPath) {
        let title = self.drive_at(&device).map_or_else(|| name::display(device.file_name()).into_owned(), Drive::title);
        if self.ask_unlock(device, &title, true) {
            return;
        }
        let k = self.tabs.items.iter().position(|t| t.id == tab).unwrap_or(self.tabs.cur);
        self.tabs.items[k].error = Some(S::ErrorsFsWrongPassphrase.t().to_owned());
        self.dirty |= k == self.tabs.cur;
    }

    pub(super) fn submit_unlock(&mut self, f: Form) -> Option<Form> {
        if f.text(1).is_empty() {
            return Some(f);
        }
        let passphrase = Secret(f.text(1).to_owned());
        let Form { kind: Kind::Unlock { device }, .. } = f else { return None };
        let (tab, launch) = (self.tab().id, Launch::Unlock { device, passphrase });
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
        None
    }
}

#[cfg(test)]
#[path = "../../test/ui/volumes.rs"]
mod tests;
