use super::archive::Kind;
use super::form::{Control, Form};
use super::ops::Then;
use super::App;
use crate::i18n::S;
use crate::name;
use crate::path::FsPath;
use crate::provider::{Access, AccessEdit, Accounts, Enclosed, FileOp, Launch, ModeEdit, OpId, OpenAsk, TabId};
use crate::undo::Origin;
use crate::{ProviderId, ProviderRequest};
use super::Effect;

const CLASSES: [S; 3] = [S::PermissionsOwner, S::PermissionsGroup, S::PermissionsOthers];
const FILE_BITS: [S; 3] = [S::PermissionsRead, S::PermissionsWrite, S::PermissionsExecute];
const FOLDER_BITS: [S; 3] = [S::PermissionsList, S::PermissionsCreate, S::PermissionsEnter];
const OWNER_EXEC: u32 = 0o100;
const LAUNCHER: &[u8] = b".desktop";

fn is_launcher(path: &FsPath) -> bool {
    let name = path.file_name();
    name.len() > LAUNCHER.len() && name[name.len() - LAUNCHER.len()..].eq_ignore_ascii_case(LAUNCHER)
}
const EXEC: u32 = 0o111;
const READ_WRITE: u32 = 0o666;

pub(super) enum Probe {
    Off,
    Waiting { items: Vec<FsPath>, held: Vec<(String, String)> },
    Editing(Editing),
}

pub(super) struct Editing {
    items: Vec<FsPath>,
    accounts: Accounts,
    at: usize,
    exec: bool,
    enclosed: bool,
    shown: u32,
    owner: String,
    group: String,
}

fn bit(class: usize, which: usize) -> u32 {
    0o400 >> (class * 3 + which)
}

fn bits(controls: &[Control], at: usize) -> u32 {
    let mut out = 0;
    for class in 0..3 {
        if let Some(Control::Flags { items, .. }) = controls.get(at + 1 + class) {
            for (which, (_, on)) in items.iter().enumerate().take(3) {
                if *on {
                    out |= bit(class, which);
                }
            }
        }
    }
    out
}

fn show_bits(controls: &mut [Control], at: usize, mode: u32) {
    for class in 0..3 {
        if let Some(Control::Flags { items, .. }) = controls.get_mut(at + 1 + class) {
            for (which, (_, on)) in items.iter_mut().enumerate().take(3) {
                *on = mode & bit(class, which) != 0;
            }
        }
    }
}

fn checked(controls: &[Control], k: Option<usize>) -> bool {
    matches!(k.and_then(|k| controls.get(k)), Some(Control::Check { on: true, .. }))
}

fn text(controls: &[Control], k: usize) -> &str {
    match controls.get(k) {
        Some(Control::Text { field, .. }) => field.text(),
        _ => "",
    }
}

fn account(typed: &str, shown: &str, known: &[(u32, String)], missing: S) -> Result<Option<u32>, String> {
    let typed = typed.trim();
    if typed.is_empty() || typed == shown {
        return Ok(None);
    }
    if let Some((id, _)) = known.iter().find(|(_, n)| n == typed) {
        return Ok(Some(*id));
    }
    typed.parse::<u32>().map(Some).map_err(|_| crate::tr!(missing, name = typed))
}

fn shared_name(before: &[Access], id: fn(&Access) -> u32, known: &[(u32, String)]) -> String {
    let first = before.first().map(id);
    match first.filter(|f| before.iter().all(|a| id(a) == *f)) {
        Some(v) => known.iter().find(|(k, _)| *k == v).map_or_else(|| v.to_string(), |(_, n)| n.clone()),
        None => String::new(),
    }
}

impl Editing {
    fn exec_at(&self) -> Option<usize> {
        self.exec.then_some(self.at + 4)
    }

    fn owner_at(&self) -> usize {
        self.at + 4 + usize::from(self.exec)
    }

    fn group_at(&self) -> usize {
        self.owner_at() + 1
    }

    fn enclosed_files_at(&self) -> Option<usize> {
        self.enclosed.then_some(self.group_at() + 1)
    }

    fn enclosed_folders_at(&self) -> Option<usize> {
        self.enclosed_files_at().map(|at| at + 4)
    }

    fn edit(&self, controls: &[Control]) -> Result<Option<AccessEdit>, String> {
        let mode = ModeEdit::between(self.shown, bits(controls, self.at));
        let owner = account(text(controls, self.owner_at()), &self.owner, &self.accounts.users, S::PermissionsNoUser)?;
        let group = account(text(controls, self.group_at()), &self.group, &self.accounts.groups, S::PermissionsNoGroup)?;
        let enclosed = self.enclosed_files_at().map(|files_at| Enclosed {
            files: ModeEdit::between(self.shown & READ_WRITE, bits(controls, files_at)),
            folders: ModeEdit::between(self.shown, bits(controls, self.enclosed_folders_at().unwrap_or_default())),
        });
        let edit = AccessEdit { mode, owner, group, enclosed };
        Ok((!edit.is_empty()).then_some(edit))
    }
}

pub(super) fn section(before: &[Access], accounts: Accounts, items: Vec<FsPath>, at: usize) -> Option<(Vec<Control>, Editing)> {
    let editable = !before.is_empty() && before.len() == items.len() && before.iter().all(|a| matches!(a.kind(), Access::FILE | Access::DIR));
    if !editable {
        return None;
    }
    let folders = before.iter().all(|a| a.kind() == Access::DIR);
    let exec = before.iter().all(|a| a.kind() == Access::FILE);
    let enclosed = before.iter().any(|a| a.kind() == Access::DIR);
    let shown = before.iter().fold(0o777, |acc, a| acc & a.mode) & 0o777;
    let labels = if folders { FOLDER_BITS } else { FILE_BITS };
    let mut controls = vec![Control::line(S::PermissionsTitle.t())];
    for (class, label) in CLASSES.iter().enumerate() {
        controls.push(Control::flags(label.t(), labels.iter().enumerate().map(|(which, l)| (l.t().to_owned(), shown & bit(class, which) != 0)).collect()));
    }
    if exec {
        controls.push(Control::check(S::PermissionsExecutable.t(), shown & OWNER_EXEC != 0));
    }
    let owner = shared_name(before, |a| a.uid, &accounts.users);
    let group = shared_name(before, |a| a.gid, &accounts.groups);
    controls.push(Control::text(S::PropertiesOwner.t(), &owner, S::PermissionsMixed.t()));
    controls.push(Control::text(S::PropertiesGroup.t(), &group, S::PermissionsMixed.t()));
    if enclosed {
        controls.push(Control::line(S::PermissionsEnclosedFiles.t()));
        for (class, label) in CLASSES.iter().enumerate() {
            controls.push(Control::flags(label.t(), FILE_BITS.iter().enumerate().map(|(which, l)| (l.t().to_owned(), shown & READ_WRITE & bit(class, which) != 0)).collect()));
        }
        controls.push(Control::line(S::PermissionsEnclosedFolders.t()));
        for (class, label) in CLASSES.iter().enumerate() {
            controls.push(Control::flags(label.t(), FOLDER_BITS.iter().enumerate().map(|(which, l)| (l.t().to_owned(), shown & bit(class, which) != 0)).collect()));
        }
    }
    Some((controls, Editing { items, accounts, at, exec, enclosed, shown, owner, group }))
}

pub(super) fn held_rows(probe: &mut Probe, controls: &mut Vec<Control>, focus: &mut usize, title: &str, rows: &[(String, String)]) {
    let access = title == S::PropertiesAccess.t();
    match probe {
        Probe::Waiting { held, .. } if access => held.extend(rows.iter().cloned()),
        Probe::Editing(_) if access => {}
        Probe::Editing(e) => {
            for (k, v) in rows {
                controls.insert(e.at, Control::value(k, v));
                e.at += 1;
                if *focus >= e.at - 1 {
                    *focus += 1;
                }
            }
        }
        _ => controls.extend(rows.iter().map(|(k, v)| Control::value(k, v))),
    }
}

impl App {
    pub(super) fn properties_access(&mut self, id: OpId, found: Vec<Option<Access>>, accounts: Accounts) {
        let Some(f) = &mut self.jobs.form else { return };
        let Form { kind: Kind::Properties(m), controls, buttons, focus, .. } = f else { return };
        if m.id != id {
            return;
        }
        let Probe::Waiting { items, held } = std::mem::replace(&mut m.probe, Probe::Off) else { return };
        let before: Option<Vec<Access>> = found.into_iter().collect();
        match before.and_then(|b| section(&b, accounts, items, controls.len())) {
            Some((added, editing)) => {
                if *focus >= editing.at {
                    *focus += added.len();
                }
                controls.extend(added);
                *buttons = vec![S::ArchiveOk.t(), S::WidgetsCancel.t()];
                m.probe = Probe::Editing(editing);
            }
            None => controls.extend(held.iter().map(|(k, v)| Control::value(k, v))),
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn permissions_changed(&mut self, k: usize) {
        let Some(Form { kind: Kind::Properties(m), controls, .. }) = &mut self.jobs.form else { return };
        let Probe::Editing(e) = &m.probe else { return };
        let mode = bits(controls, e.at);
        match e.exec_at() {
            Some(x) if x == k => {
                let readable = |class: usize| mode & bit(class, 0) != 0;
                let shown = if checked(controls, Some(x)) { mode | OWNER_EXEC | if readable(1) { 0o010 } else { 0 } | if readable(2) { 0o001 } else { 0 } } else { mode & !EXEC };
                show_bits(controls, e.at, shown);
            }
            Some(x) if (e.at + 1..e.at + 4).contains(&k) => {
                if let Some(Control::Check { on, .. }) = controls.get_mut(x) {
                    *on = mode & OWNER_EXEC != 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn submit_properties(&mut self, mut f: Form) -> Option<Form> {
        let Kind::Properties(m) = &f.kind else { return None };
        let Probe::Editing(e) = &m.probe else {
            self.form_closed(f);
            return None;
        };
        match e.edit(&f.controls) {
            Ok(edit) => {
                if let Some(edit) = edit {
                    self.start_op(FileOp::SetAccess { items: e.items.clone(), edit }, Origin::Fresh, Then::Nothing);
                }
                self.form_closed(f);
                None
            }
            Err(message) => {
                f.say(message, true);
                Some(f)
            }
        }
    }

    pub fn warn_elevated(&mut self) {
        self.status(if self.caps.unix_paths { S::UiAppRoot.t() } else { S::UiAppElevated.t() });
    }

    pub(super) fn ask_allow_run(&mut self, tab: TabId, path: FsPath) {
        if self.jobs.form.is_some() {
            return;
        }
        let shown = crate::tr!(S::UiQuoted, name = name::display(path.file_name()));
        let launcher = is_launcher(&path);
        let hint = if launcher { S::PermissionsLauncherHint.t() } else { S::PermissionsRunHint.t() };
        let controls = vec![Control::line(&crate::tr!(S::PermissionsNotExecutable, name = shown)), Control::line(hint)];
        let title = crate::tr!(S::PermissionsRunTitle, name = shown);
        let buttons = if launcher { vec![S::PermissionsAllowRun.t(), S::WidgetsCancel.t()] } else { vec![S::PermissionsAllowRun.t(), S::ActionsFileRunInTerminal.t(), S::WidgetsCancel.t()] };
        self.open_form(Form::new(Kind::AllowRun { tab, path }, &title, controls, buttons));
    }

    pub(super) fn open_ask(&mut self, tab: TabId, path: FsPath, ask: OpenAsk) {
        match ask {
            OpenAsk::NoApp { mime, apps } => self.choose_app(vec![path], mime, apps),
            OpenAsk::Script if self.jobs.form.is_none() => {
                let shown = crate::tr!(S::UiQuoted, name = name::display(path.file_name()));
                let controls = vec![Control::line(&crate::tr!(S::PermissionsScript, name = shown)), Control::line(S::PermissionsScriptHint.t())];
                let title = crate::tr!(S::PermissionsRunTitle, name = shown);
                let buttons = vec![S::PermissionsRun.t(), S::ActionsFileRunInTerminal.t(), S::MenuOpen.t(), S::WidgetsCancel.t()];
                self.open_form(Form::new(Kind::RunScript { tab, path }, &title, controls, buttons));
            }
            OpenAsk::Script => {}
        }
    }

    pub(super) fn submit_run_script(&mut self, f: Form, b: usize) -> Option<Form> {
        if let Kind::RunScript { tab, path } = f.kind {
            let launch = match b {
                0 => Launch::Execute(path),
                1 => Launch::RunInTerminal { program: path },
                _ => Launch::Display(path),
            };
            self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
        }
        None
    }

    pub(super) fn submit_allow_run(&mut self, f: Form, b: usize) -> Option<Form> {
        if let Kind::AllowRun { tab, path } = f.kind {
            let edit = AccessEdit { mode: ModeEdit { set: OWNER_EXEC, clear: 0 }, ..Default::default() };
            let then = match b {
                1 => Then::Terminal,
                _ => Then::Launch,
            };
            self.start_op_for(tab, FileOp::SetAccess { items: vec![path], edit }, Origin::Fresh, then);
        }
        None
    }
}

#[cfg(test)]
#[path = "../../test/ui/permissions.rs"]
mod tests;
