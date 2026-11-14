use super::archive::Kind;
use super::form::{Control, Form};
use super::menu::Menu;
use super::sidebar::{Drive, PlaceCmd};
use super::{App, Effect};
use crate::archive::Secret;
use crate::i18n::S;
use crate::location::ProviderId;
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, ProviderRequest};

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

    pub(super) fn unmounted_drive(&self, path: &FsPath) -> bool {
        self.drive_at(path).is_some_and(|d| d.unmounted)
    }

    pub(super) fn mount_drive(&mut self, path: &FsPath) -> bool {
        let Some(d) = self.drive_at(path).filter(|d| d.unmounted) else { return false };
        if d.locked() {
            let (device, title) = (d.path.clone(), d.title());
            self.ask_unlock(device, &title);
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
        if d.mountable() {
            m.place(if d.locked() { S::SidebarDriveUnlock.t() } else { S::MenuMount.t() }, PlaceCmd::Mount(d.path.clone()), false);
        }
    }

    pub(super) fn computer_drive_items(&self, m: &mut Menu) {
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|v| t.view_loc.path().kind() == PathKind::Computer && v.selected_count() == 1) else { return };
        let Some(i) = v.selected().next() else { return };
        let path = t.entry_location(i).path().clone();
        if self.drive_at(&path).is_some_and(|d| d.removable && !d.unmounted && d.path.kind() == PathKind::Unix) {
            m.place(S::MenuEject.t(), PlaceCmd::Eject(path.clone()), false);
        }
        self.drive_items(m, &path);
    }

    pub(super) fn ask_unlock(&mut self, device: FsPath, title: &str) {
        if self.jobs.form.is_some() {
            return;
        }
        let controls = vec![Control::value(S::SidebarDriveVolume.t(), title), Control::secret(S::SidebarDrivePassphrase.t(), "")];
        self.open_form(Form::new(Kind::Unlock { device }, S::SidebarDriveUnlockTitle.t(), controls, vec![S::SidebarDriveUnlock.t(), S::SettingsPageDialogCancel.t()]));
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
