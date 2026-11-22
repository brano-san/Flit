use super::archive::Kind;
use super::form::{Control, Form};
use super::palette::{Palette, Pick};
use super::{App, Effect, Route, SavedServer};
use crate::i18n::S;
use crate::location::{Location, ProviderId};
use crate::open::OpenTarget;
use std::time::Instant;
use crate::open::Open;
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, LoginAsk, MountAnswer, MountAsk, MountReply, Password, ProviderRequest, ASK_ANONYMOUS, ASK_DOMAIN, ASK_PASSWORD, ASK_SAVING, ASK_USER};
use crate::render::{Color, Rect};
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::provider::NetworkWarning;
use crate::theme::Theme;

const SCHEMES: [&str; 11] = ["smb", "sftp", "ssh", "ftp", "ftps", "dav", "davs", "dav+sd", "davs+sd", "afp", "nfs"];
const SHARING_SETTINGS: &str = "ms-settings:network-advancedsharing";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DiscoveryCheck {
    #[default]
    Unchecked,
    Pending(u64),
    Checked(Option<NetworkWarning>),
}

fn empty_network(snap: &crate::listing::DirSnapshot) -> bool {
    snap.path.is_network() && snap.path.is_root() && snap.done && snap.listing.is_empty()
        && snap.error.as_ref().is_none_or(|error| error.code == crate::listing::NO_NETWORK_BROWSER_CODE)
}

pub fn server_uri(text: &str) -> Option<String> {
    let text = text.trim();
    let (scheme, rest) = text.split_once("://")?;
    let host = rest.split('/').next()?.rsplit('@').next()?;
    let clean = !text.contains('\\') && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    (SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) && !host.is_empty() && clean).then(|| text.to_owned())
}

pub(super) fn windows_ftp(text: &str) -> bool {
    text.trim().get(..6).is_some_and(|scheme| scheme.eq_ignore_ascii_case("ftp://"))
}

pub fn saved_address(text: &str) -> Option<String> {
    let uri = server_uri(text)?;
    let (scheme, rest) = uri.split_once("://")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if rest.contains(['?', '#']) {
        return None;
    }
    let (login, host) = authority.rsplit_once('@').unwrap_or(("", authority));
    let user = login.split(':').next().unwrap_or_default();
    if user.contains('@') || host.is_empty() {
        return None;
    }
    let mut address = format!("{}://", scheme.to_ascii_lowercase());
    if !user.is_empty() {
        address.push_str(user);
        address.push('@');
    }
    address.push_str(&host.to_lowercase());
    address.push_str(path.trim_end_matches('/'));
    matches!(crate::redact::passwords(&address), std::borrow::Cow::Borrowed(_)).then_some(address)
}

pub(super) fn server_label(uri: &str) -> &str {
    let rest = uri.split_once("://").map_or(uri, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    authority.rsplit_once(':').filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).map_or(authority, |(label, _)| label)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerChange {
    Add(String),
    Rename(String, Option<String>),
    Remove(String),
}

impl ServerChange {
    pub fn apply(&self, servers: &mut Vec<SavedServer>) -> bool {
        match self {
            Self::Add(address) if !servers.iter().any(|s| &s.address == address) => {
                servers.push(SavedServer { address: address.clone(), name: None });
            }
            Self::Rename(address, name) => {
                let Some(server) = servers.iter_mut().find(|s| &s.address == address && &s.name != name) else { return false };
                server.name.clone_from(name);
            }
            Self::Remove(address) => {
                let Some(i) = servers.iter().position(|s| &s.address == address) else { return false };
                servers.remove(i);
            }
            Self::Add(_) => return false,
        }
        true
    }
}

pub(super) fn network_entry(name: &[u8]) -> Option<Location> {
    let uri = server_uri(std::str::from_utf8(name).ok()?)?;
    FsPath::network_server(uri.as_bytes()).map(Location::from_local)
}

pub(super) fn network_address(path: &FsPath) -> Option<String> {
    (path.is_network() && !path.is_root()).then_some(())?;
    let mut parts = path.components();
    let uri = std::str::from_utf8(parts.next()?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    server_uri(uri)
}

pub(super) fn native_url(text: &str) -> bool {
    let scheme = text.split_once(':').map_or("", |(s, _)| s);
    ["file", "trash", "recent"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) || text.eq_ignore_ascii_case(crate::path::NETWORK_URI) || server_uri(text).is_some()
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
    pub(super) fn check_discovery(&mut self, k: usize, snap: &crate::listing::DirSnapshot) {
        let tab = &mut self.tabs.items[k];
        if tab.discovery == DiscoveryCheck::Unchecked && empty_network(snap) {
            tab.discovery = DiscoveryCheck::Pending(snap.gen);
            self.effects.push(Effect::NetworkDiscovery { tab: tab.id, gen: snap.gen });
        }
    }

    pub(super) fn discovery_warning(&self) -> Option<NetworkWarning> {
        let tab = self.tab();
        let DiscoveryCheck::Checked(warning) = tab.discovery else { return None };
        let path = tab.location().path();
        let view = tab.view.as_ref().filter(|view| view.snap().gen == tab.gen && &view.snap().path == path && empty_network(view.snap()));
        view.and(warning)
    }

    pub(super) fn network_settings_rect(&self) -> Option<Rect> {
        let r = self.layout.list;
        let width = self.cs(190.0).min(r.w - self.cs(24.0));
        (self.discovery_warning() == Some(NetworkWarning::WindowsDisabled) && width > 0.0 && r.h >= self.cs(106.0))
            .then(|| Rect::new(r.x + (r.w - width) * 0.5, r.y + self.cs(62.0), width, self.cs(28.0)))
    }

    pub(super) fn open_network_settings(&mut self) {
        if self.network_settings_rect().is_some() {
            self.launch(Launch::Run { program: SHARING_SETTINGS.into(), args: String::new(), dir: None });
        }
    }

    pub(super) fn draw_discovery_warning<B: TextBackend>(&self, g: &mut Gfx<B>, theme: &Theme, list: Rect, warning: NetworkWarning) {
        let font = self.content_font(13.0);
        let banner = list.inset(self.cs(8.0), self.cs(8.0));
        let banner = Rect { h: self.cs(90.0).min(banner.h), ..banner };
        g.dl.round_rect(banner, theme.button_hover, self.cs(4.0));
        let message = match warning {
            NetworkWarning::WindowsDisabled => S::UiEmptyNetworkDiscoveryDisabled.t().to_owned(),
            NetworkWarning::LinuxMissing { avahi, gvfs } => [
                avahi.map(|package| crate::tr!(S::UiEmptyNetworkStartAvahi, package = package)),
                gvfs.map(|package| crate::tr!(S::UiEmptyNetworkInstallGvfs, package = package)),
            ].into_iter().flatten().collect::<Vec<_>>().join(" "),
        };
        self.draw_network_notice(g, Rect { y: banner.y + self.cs(20.0), ..banner }, &message, font, theme.text);
        if let Some(button) = self.network_settings_rect() {
            let color = if self.hover == super::Hit::NetworkSettings { theme.button_pressed } else { theme.bg };
            g.dl.round_rect(button, color, self.cs(4.0));
            super::centered(g, button, S::UiButtonOpenNetworkSettings.t(), font, theme.text);
        }
    }

    pub(super) fn draw_network_notice<B: TextBackend>(&self, g: &mut Gfx<B>, area: Rect, text: &str, font: FontStyle, color: Color) {
        let width = (area.w - self.cs(24.0)).max(1.0);
        for (line, text) in super::update_dialog::wrapped(g, text, font, width).into_iter().enumerate() {
            let w = g.measure(&text, font).min(width);
            g.text(area.x + (area.w - w) / 2.0, area.y + self.cs(20.0) * line as f32, &text, font, color, width);
        }
    }

    pub(super) fn map_drive(&mut self) {
        let share = self.tab().view_loc.to_windows().filter(|p| p.kind() == PathKind::Unc);
        self.launch(Launch::MapDrive(share));
    }

    pub(super) fn ask_server(&mut self) {
        if self.caps.unix_paths {
            self.server_form(false);
        } else {
            self.open_pick(Palette::new(Pick::Server, Vec::new(), ""));
        }
    }

    pub(super) fn ask_saved_server(&mut self) {
        self.server_form(true);
    }

    fn server_form(&mut self, save: bool) {
        let controls = vec![Control::text(S::A11yRegionAddress.t(), "", S::SidebarDriveServerExample.t()), Control::check(S::SidebarDriveSaveServer.t(), save)];
        let mut form = Form::new(Kind::ConnectServer, S::SidebarDriveConnectServer.t(), controls, vec![S::SidebarDriveLoginConnect.t(), S::WidgetsCancel.t()]);
        form.say(S::SidebarDriveServerPrompt.t(), false);
        self.open_form(form);
    }

    pub(super) fn submit_server(&mut self, mut form: Form) -> Option<Form> {
        let launch = if form.checked(1) {
            saved_address(form.text(0)).map(Launch::ConnectAndSaveServer)
        } else {
            server_uri(form.text(0)).map(Launch::ConnectServer)
        };
        let Some(launch) = launch else {
            form.say(S::SidebarDriveServerPrompt.t(), true);
            return Some(form);
        };
        self.launch(launch);
        None
    }

    pub(super) fn ask_rename_server(&mut self, address: String) {
        let Some(server) = self.nav.sidebar.prefs.saved_servers.iter().find(|s| s.address == address) else { return };
        let name = server.name.as_deref().unwrap_or_else(|| server_label(&address));
        let controls = vec![Control::text(S::DetailsColumnName.t(), name, "")];
        self.open_form(Form::new(Kind::RenameServer(address), S::MenuRename.t(), controls, vec![S::MenuRename.t(), S::WidgetsCancel.t()]));
    }

    pub(super) fn submit_server_name(&mut self, form: Form) -> Option<Form> {
        let text = form.text(0).trim();
        let name = (!text.is_empty()).then(|| text.to_owned());
        if let Kind::RenameServer(address) = form.kind {
            self.change_server(ServerChange::Rename(address, name));
        }
        None
    }

    pub(super) fn remove_saved_server(&mut self, address: &str) {
        self.change_server(ServerChange::Remove(address.to_owned()));
    }

    pub fn apply_server_change(&mut self, change: &ServerChange) -> bool {
        if self.caps.unix_paths && change.apply(&mut self.nav.sidebar.prefs.saved_servers) {
            self.refresh_saved_servers();
            return true;
        }
        false
    }

    pub(super) fn change_server(&mut self, change: ServerChange) {
        if self.apply_server_change(&change) {
            self.effects.push(Effect::SavedServer(change));
        }
    }

    pub(super) fn connect_server(&mut self, text: &str) {
        let share = (!self.caps.unix_paths).then(|| text.trim()).filter(|t| FsPath::parse(t).is_some_and(|p| p.kind() == PathKind::Unc)).map(str::to_owned);
        match server_uri(text).or(share) {
            Some(uri) => self.launch(Launch::ConnectServer(uri)),
            None => self.status(S::SidebarDriveServerPrompt.t()),
        }
    }

    pub(super) fn open_network(&mut self, i: u32, now: Instant) {
        let Some(folder) = self.tab().entry_location(i) else { return };
        if network_address(folder.path()).is_some() {
            self.dispatch_entries(vec![Route { folder, select: None }], OpenTarget::CurrentTab, now);
        }
    }

    pub fn activate_network_tabs(&mut self) {
        for k in 0..self.tabs.items.len() {
            self.begin_network(k);
        }
    }

    pub(super) fn begin_network(&mut self, k: usize) {
        let t = &mut self.tabs.items[k];
        let Some(uri) = network_address(t.location().path()).filter(|_| self.caps.unix_paths) else { return };
        t.slot.take();
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab: t.id, launch: Launch::ConnectNetwork { uri, gen: t.gen } } });
    }

    pub(super) fn network_connected(&mut self, k: usize, result: Result<Option<FsPath>, String>, now: Instant) {
        match result {
            Ok(Some(path)) if self.tabs.items[k].location().path() == &path => {
                self.tabs.items[k].list_network(self.case, &mut self.effects);
            }
            Ok(Some(path)) => {
                self.tabs.items[k].history.replace(Location::from_local(path));
                self.tabs.items[k].drop_trail();
                self.load_tab(k, None, now);
                if k == self.tabs.cur {
                    self.after_tab_change();
                } else {
                    self.tabs.strip.rev += 1;
                }
            }
            result => {
                self.load_tab(k, None, now);
                if let Err(error) = result {
                    self.tabs.items[k].error = Some(crate::redact::passwords(&crate::i18n::localize(&error)).into_owned());
                }
            }
        }
    }

    pub(super) fn mount_ask(&mut self, ask: MountAsk, answer: MountAnswer) {
        if self.dialogs.form.is_some() {
            return;
        }
        let form = match ask {
            MountAsk::Login(ask) => login_form(ask, answer),
            MountAsk::Question { choices, .. } if choices.is_empty() => {
                answer.send(MountReply::Cancel);
                return;
            }
            MountAsk::Question { server, message, choices } => {
                let mut controls: Vec<Control> = message_lines(&crate::redact::passwords(&message)).iter().map(|l| Control::line(l)).collect();
                let at = controls.len();
                controls.push(Control::choice(S::SidebarDriveLoginAnswer.t(), choices, 0));
                let title = crate::tr!(S::SidebarDriveLoginTitle, server = crate::redact::passwords(&server));
                Form::new(Kind::MountQuestion(Question { answer, at }), &title, controls, vec![S::SidebarDriveLoginContinue.t(), S::WidgetsCancel.t()])
            }
        };
        self.open_form(form);
    }

    pub(super) fn mount_ask_done(&mut self, id: u64) {
        let ours = self.dialogs.form.as_ref().is_some_and(|f| matches!(&f.kind, Kind::Login(l) if l.answer.id() == id) || matches!(&f.kind, Kind::MountQuestion(q) if q.answer.id() == id));
        if let Some(mut f) = ours.then(|| self.dialogs.form.take()).flatten() {
            f.wipe_secrets();
            self.show_conflict_table();
            self.layout();
            self.dirty = true;
        }
    }

    pub(super) fn submit_login(&mut self, mut f: Form) -> Option<Form> {
        let Kind::Login(l) = &f.kind else { return None };
        let anonymous = l.anonymous.is_some_and(|k| f.checked(k));
        let user = l.user.map_or_else(|| l.default_user.clone(), |k| f.text(k).trim().to_owned());
        let domain = l.domain.map_or_else(|| l.default_domain.clone(), |k| f.text(k).trim().to_owned());
        let password = Password::new(l.password.map(|k| f.text(k).to_owned()).unwrap_or_default());
        let remember = l.remember.is_some_and(|k| f.choice(k) == 1);
        f.wipe_secrets();
        if let Kind::Login(l) = f.kind {
            l.answer.send(MountReply::Login { user, domain, password, anonymous, remember });
        }
        None
    }

    pub(super) fn submit_mount_question(&mut self, f: Form) -> Option<Form> {
        let choice = match &f.kind {
            Kind::MountQuestion(q) => f.choice(q.at),
            _ => return None,
        };
        if let Kind::MountQuestion(q) = f.kind {
            q.answer.send(MountReply::Choice(choice));
        }
        None
    }
}

pub(super) struct Login {
    pub answer: MountAnswer,
    default_user: String,
    default_domain: String,
    user: Option<usize>,
    domain: Option<usize>,
    password: Option<usize>,
    anonymous: Option<usize>,
    remember: Option<usize>,
}

pub(super) struct Question {
    pub answer: MountAnswer,
    at: usize,
}

pub(super) fn mount_form_closed(mut f: Form) {
    f.wipe_secrets();
    match f.kind {
        Kind::Login(l) => {
            l.answer.send(MountReply::Cancel);
        }
        Kind::MountQuestion(q) => {
            q.answer.send(MountReply::Cancel);
        }
        _ => {}
    }
}

const LINE_CHARS: usize = 72;

pub(super) fn message_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > LINE_CHARS {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn login_form(ask: LoginAsk, answer: MountAnswer) -> Form {
    let mut controls: Vec<Control> = message_lines(&crate::redact::passwords(&ask.message)).iter().map(|l| Control::line(l)).collect();
    let mut add = |wanted: bool, c: Control| {
        wanted.then(|| {
            controls.push(c);
            controls.len() - 1
        })
    };
    let user = add(ask.wants(ASK_USER), Control::text(S::SidebarDriveLoginUser.t(), &ask.user, ""));
    let domain = add(ask.wants(ASK_DOMAIN), Control::text(S::SidebarDriveLoginDomain.t(), &ask.domain, ""));
    let password = add(ask.wants(ASK_PASSWORD), Control::secret(S::SidebarDriveLoginPassword.t(), ""));
    let anonymous = add(ask.wants(ASK_ANONYMOUS), Control::check(S::SidebarDriveLoginAnonymous.t(), false));
    let remember = add(ask.wants(ASK_SAVING), Control::choice(S::SidebarDriveLoginRemember.t(), vec![S::SidebarDriveLoginForget.t().to_owned(), S::SidebarDriveLoginSession.t().to_owned()], 0));
    let title = crate::tr!(S::SidebarDriveLoginTitle, server = crate::redact::passwords(&ask.server));
    let login = Login { answer, default_user: ask.user, default_domain: ask.domain, user, domain, password, anonymous, remember };
    let mut form = Form::new(Kind::Login(login), &title, controls, vec![S::SidebarDriveLoginConnect.t(), S::WidgetsCancel.t()]);
    if let Some(k) = password.filter(|_| user.is_none_or(|u| !form.text(u).is_empty())) {
        form.focus = k;
    }
    form
}

#[cfg(test)]
#[path = "../../test/ui/network.rs"]
mod tests;
