use super::archive::Kind;
use super::form::{Control, Form};
use super::{App, Column, Effect, Tab};
use crate::fmt as format;
use crate::git::{GitEvent, GitInfo, GitOp, GitRequest, GitStatus};
use crate::i18n::S;
use crate::location::{Location, ProviderId};
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, TabId};
use crate::render::Color;
use std::sync::Arc;

const COMMIT_COLUMNS: u8 = 0b11110;

pub(super) fn status_color(s: GitStatus) -> Color {
    match s {
        GitStatus::Modified | GitStatus::Renamed => Color::rgb(0xD0_8A_1E),
        GitStatus::Added | GitStatus::Untracked => Color::rgb(0x3F_A3_4D),
        GitStatus::Deleted | GitStatus::Conflict => Color::rgb(0xD1_3B_3B),
    }
}

fn status_name(s: GitStatus) -> &'static str {
    match s {
        GitStatus::Modified => S::GitStateModified,
        GitStatus::Added => S::GitStateAdded,
        GitStatus::Deleted => S::GitStateDeleted,
        GitStatus::Renamed => S::GitStateRenamed,
        GitStatus::Untracked => S::GitStateUntracked,
        GitStatus::Conflict => S::GitStateConflict,
    }
    .t()
}

fn git_dir(loc: &Location) -> Option<FsPath> {
    if loc.provider() != ProviderId::Local || crate::archive::split(loc.path()).is_some() {
        return None;
    }
    matches!(loc.path().kind(), PathKind::Drive | PathKind::Unix).then(|| loc.path().clone())
}

impl Tab {
    pub(super) fn keep_git_root(&mut self) {
        let loc = self.location();
        let root = self.git.as_ref().and_then(|g| g.root.clone()).filter(|r| loc.path().is_inside(r) || loc.path().same(r));
        self.git = root.map(|root| {
            let branch = self.git.as_ref().map(|g| g.branch.clone()).unwrap_or_default();
            Arc::new(GitInfo { root: Some(root), branch, ..GitInfo::default() })
        });
    }

    pub(super) fn git_status(&self, name: &[u8]) -> Option<GitStatus> {
        self.git.as_ref()?.status.get(name).copied()
    }
}

impl App {
    pub(super) fn set_git_integration(&mut self, on: bool) {
        self.prefs.git_integration = on;
        self.each_pane(|a, k| {
            a.tabs[k].git = None;
            if let Some(v) = &mut a.tabs[k].view {
                v.set_git(None);
            }
            if a.tabs[k].view_spec.sort.field == crate::sort::SortField::GitHash {
                a.resort(k);
            }
            if on { a.git_refresh(k, false); }
        });
        self.layout();
        self.dirty = true;
    }

    pub(super) fn git_refresh(&mut self, k: usize, changed: bool) {
        if !self.prefs.git_integration { return; }
        let t = &self.tabs[k];
        if changed && t.git.as_ref().is_none_or(|g| g.root.is_none()) {
            return;
        }
        let Some(dir) = git_dir(t.location()) else { return };
        let commits = t.columns.git & COMMIT_COLUMNS != 0 || t.view_spec.sort.field == crate::sort::SortField::GitHash;
        self.effects.push(Effect::Git(GitRequest::Status { tab: t.id, gen: t.gen, dir, commits, changed }));
    }

    pub(super) fn git_text(&self, s: &mut impl std::fmt::Write, name: &[u8], col: Column) {
        let Some(g) = &self.tab().git else { return };
        if col == Column::GitStatus {
            if let Some(st) = g.status.get(name) {
                let _ = s.write_str(status_name(*st));
            }
            return;
        }
        let Some(c) = g.commits.get(name) else { return };
        let _ = match col {
            Column::GitMessage => s.write_str(&c.message),
            Column::GitAuthor => s.write_str(&c.author),
            Column::GitHash => s.write_str(&c.short_hash),
            _ => {
                let mut d = String::new();
                format::datetime_in(&mut d, c.time, self.utc_offset_min, &self.style);
                s.write_str(&d)
            }
        };
    }

    pub fn git_branch(&self) -> Option<&str> {
        self.tab().git.as_ref().filter(|g| g.root.is_some()).map(|g| g.branch.as_str())
    }

    pub fn git_info(&self) -> Option<&GitInfo> {
        self.tab().git.as_deref()
    }

    pub(super) fn selected_commit_hash(&self) -> Option<&str> {
        let t = self.tab();
        if !t.columns.shown(Column::GitHash) || git_dir(t.location()).is_none() {
            return None;
        }
        let g = t.git.as_ref().filter(|g| g.root.is_some())?;
        let v = t.view.as_ref().filter(|v| v.selected_count() == 1)?;
        let name = v.listing().name(v.selected().next()?);
        g.commits.get(name).map(|c| c.hash.as_str()).filter(|s| !s.is_empty())
    }

    pub fn on_git(&mut self, ev: GitEvent) {
        match ev {
            GitEvent::Status { tab, gen, info } => self.git_status_arrived(tab, gen, info),
            GitEvent::Branches { tab, gen, dir, create, branches } => {
                if self.tab().id == tab && self.tab().gen == gen && git_dir(self.tab().location()).as_ref() == Some(&dir) && self.jobs.form.is_none() {
                    self.git_branches = branches;
                    if create {
                        self.new_git_branch_form(tab, dir);
                    } else {
                        let r = self.layout.branch;
                        let menu = self.build_branch_menu();
                        self.flit_menu(r.x, r.y, menu);
                    }
                }
            }
            GitEvent::Started { tab, op } => {
                if self.tab().id == tab {
                    self.status(started_text(&op));
                }
            }
            GitEvent::Done { tab, op, error, url } => {
                let here = self.tab().id == tab;
                let handled = self.git_branch_done(tab, &op, error.as_deref());
                match (error, url) {
                    (Some(e), _) if here && !handled => self.git_failed(tab, &op, &e),
                    (None, Some(url)) => self.launch(Launch::Run { program: url, args: String::new(), dir: None }),
                    (None, None) if here => self.status(done_text(&op)),
                    _ => {}
                }
                for k in 0..self.tabs.len() {
                    self.git_refresh(k, true);
                }
                if op == GitOp::Init {
                    self.git_refresh(self.cur, false);
                }
            }
        }
    }

    fn git_status_arrived(&mut self, tab: TabId, gen: u64, info: Option<Arc<GitInfo>>) {
        if !self.prefs.git_integration { return; }
        let info = info.filter(|g| g.root.is_some());
        if let Some(k) = self.tabs.iter().position(|t| t.id == tab && t.gen == gen) {
            self.tabs[k].git = info;
            if self.tabs[k].view_spec.sort.field == crate::sort::SortField::GitHash {
                self.resort(k);
            }
            if k == self.cur {
                self.layout();
                self.dirty = true;
            }
            return;
        }
        if let Some(id) = self.parked_of(tab) {
            self.visit(id, |a| a.git_status_arrived(tab, gen, info));
        }
    }

    fn git_target(&mut self, need_repo: bool) -> Option<(TabId, FsPath)> {
        let t = self.tab();
        let dir = git_dir(t.location());
        let id = t.id;
        let in_repo = t.git.as_ref().is_some_and(|g| g.root.is_some());
        match dir {
            Some(d) if in_repo || !need_repo => Some((id, d)),
            _ => {
                self.status(S::GitErrorNotRepo.t());
                None
            }
        }
    }

    pub(super) fn git_run(&mut self, op: GitOp) {
        if op == GitOp::Sync && !self.git_can_sync() {
            return self.status(S::GitErrorNoUpstream.t());
        }
        let need_repo = !matches!(op, GitOp::Init | GitOp::Clone { .. });
        if let Some((tab, dir)) = self.git_target(need_repo) {
            self.effects.push(Effect::Git(GitRequest::Run { tab, dir, op }));
        }
    }

    pub(super) fn git_branches_menu(&mut self) {
        if let Some((tab, dir)) = self.git_target(true) {
            self.effects.push(Effect::Git(GitRequest::Branches { tab, gen: self.tab().gen, dir, create: false }));
        }
    }

    pub(super) fn git_can_sync(&self) -> bool {
        git_dir(self.tab().location()).is_some() && self.git_info().is_some_and(|g| g.root.is_some() && g.upstream.is_some())
    }

    pub(super) fn git_switch(&mut self, k: usize) {
        if let Some(b) = self.git_branches.get(k).cloned() {
            self.git_run(GitOp::Switch(b.name));
        }
    }

    pub(super) fn ask_git_branch(&mut self) {
        if self.jobs.form.is_some() { return; }
        if let Some((tab, dir)) = self.git_target(true) {
            self.effects.push(Effect::Git(GitRequest::Branches { tab, gen: self.tab().gen, dir, create: true }));
        }
    }

    fn new_git_branch_form(&mut self, tab: TabId, dir: FsPath) {
        let current = self.git_branch().unwrap_or_default();
        let mut bases: Vec<String> = self.git_branches.iter().filter(|b| !b.remote).map(|b| b.name.clone()).collect();
        if let Some(at) = bases.iter().position(|b| b == current) {
            bases[..=at].rotate_right(1);
        } else {
            bases.insert(0, "HEAD".into());
        }
        let controls = vec![Control::text(S::GitBranchName.t(), "", ""), Control::choice(S::GitBranchBase.t(), bases.clone(), 0), Control::check(S::GitBranchCheckout.t(), true)];
        self.open_form(Form::new(Kind::GitNewBranch { tab, dir, bases, pending: None }, S::ActionsGitNewBranch.t(), controls, vec![S::GitBranchCreate.t(), S::WidgetsCancel.t()]));
    }

    pub(super) fn submit_git_branch(&mut self, mut f: Form) -> Option<Form> {
        let name = f.text(0).to_owned();
        let choice = f.choice(1);
        let switch = f.checked(2);
        if let Kind::GitNewBranch { tab, dir, bases, pending } = &mut f.kind {
            if pending.is_none() {
                let Some(base) = bases.get(choice).cloned() else { return Some(f) };
                let op = GitOp::CreateBranch { name, base, switch };
                *pending = Some(op.clone());
                self.effects.push(Effect::Git(GitRequest::Run { tab: *tab, dir: dir.clone(), op }));
                f.say(S::GitBranchCreating.t(), false);
            }
        }
        Some(f)
    }

    pub(super) fn ask_git_delete(&mut self, k: usize) {
        if self.jobs.form.is_some() { return; }
        let Some(b) = self.git_branches.get(k).filter(|b| !b.remote && Some(b.name.as_str()) != self.git_branch()).cloned() else { return };
        let Some((tab, dir)) = self.git_target(true) else { return };
        let controls = vec![Control::paragraph(crate::tr!(S::GitBranchConfirm, branch = b.name))];
        let mut f = Form::new(Kind::GitDeleteBranch { tab, dir, name: b.name, force: false, pending: false }, S::GitBranchDelete.t(), controls, vec![S::GitBranchDelete.t(), S::WidgetsCancel.t()]);
        f.focus = f.controls.len() + 1;
        self.open_form(f);
    }

    pub(super) fn submit_git_delete(&mut self, mut f: Form) -> Option<Form> {
        if let Kind::GitDeleteBranch { tab, dir, name, force, pending } = &mut f.kind {
            if !*pending {
                *pending = true;
                self.effects.push(Effect::Git(GitRequest::Run { tab: *tab, dir: dir.clone(), op: GitOp::DeleteBranch { name: name.clone(), force: *force } }));
                f.say(S::GitBranchDeleting.t(), false);
            }
        }
        Some(f)
    }

    fn git_branch_done(&mut self, tab: TabId, op: &GitOp, error: Option<&str>) -> bool {
        let Some(mut f) = self.jobs.form.take() else { return false };
        let handled = match &mut f.kind {
            Kind::GitNewBranch { tab: t, pending, .. } if *t == tab && pending.as_ref() == Some(op) => {
                *pending = None;
                if let Some(e) = error { f.say(e, true); }
                true
            }
            Kind::GitDeleteBranch { tab: t, name, force, pending, .. } if *t == tab && *pending && *op == (GitOp::DeleteBranch { name: name.clone(), force: *force }) => {
                *pending = false;
                if let Some(e) = error {
                    if !*force && e.contains("not fully merged") {
                        *force = true;
                        f.controls = vec![Control::paragraph(crate::tr!(S::GitBranchForceConfirm, branch = name))];
                        f.note.clear();
                        f.error = false;
                        f.focus = f.controls.len() + 1;
                    } else { f.say(e, true); }
                }
                true
            }
            _ => false,
        };
        if !handled || error.is_some() { self.jobs.form = Some(f); }
        if handled { self.layout(); self.dirty = true; }
        handled
    }

    pub(super) fn ask_clone(&mut self) {
        if self.git_target(false).is_none() || self.jobs.form.is_some() {
            return;
        }
        let controls = vec![Control::text(S::GitCloneUrl.t(), "", S::GitCloneHint.t())];
        self.open_form(Form::new(Kind::GitClone, S::GitCloneTitle.t(), controls, vec![S::GitCloneButton.t(), S::SettingsPageDialogCancel.t()]));
    }

    pub(super) fn submit_clone(&mut self, f: Form) -> Option<Form> {
        let url = f.text(0).trim().to_owned();
        if crate::git::repo_name(&url).is_none() {
            return Some(f);
        }
        self.git_run(GitOp::Clone { url });
        None
    }
}

fn started_text(op: &GitOp) -> String {
    match op {
        GitOp::Fetch => S::GitOpFetching.t().to_owned(),
        GitOp::Pull => S::GitOpPulling.t().to_owned(),
        GitOp::Push => S::GitOpPushing.t().to_owned(),
        GitOp::Sync => S::GitOpSyncing.t().to_owned(),
        GitOp::Init => S::GitOpInitializing.t().to_owned(),
        GitOp::Clone { url } => crate::tr!(S::GitOpCloning, url = url),
        GitOp::Switch(b) => crate::tr!(S::GitOpSwitching, branch = b),
        GitOp::CreateBranch { .. } => S::GitBranchCreating.t().into(),
        GitOp::DeleteBranch { .. } => S::GitBranchDeleting.t().into(),
        GitOp::OpenRemote => String::new(),
    }
}

impl App {
    fn git_failed(&mut self, tab: TabId, op: &GitOp, e: &str) {
        match crate::git::failure(e) {
            crate::git::Failure::Missing => self.status(if self.caps.unix_paths { S::GitErrorInstallLinux } else { S::GitErrorInstallWindows }.t()),
            crate::git::Failure::Auth if self.jobs.form.is_none() => {
                let (Some(command), Some(dir)) = (crate::git::op_command(op), self.git_dir_of(tab)) else { return self.status(crate::tr!(S::GitErrorFailed, error = e)) };
                let controls = vec![Control::line(&crate::tr!(S::GitErrorAuthText, command = command)), Control::line(e)];
                self.open_form(Form::new(Kind::GitAuth { dir }, S::GitErrorAuthTitle.t(), controls, vec![S::GitErrorAuthOpen.t(), S::WidgetsCancel.t()]));
            }
            _ => self.status(crate::tr!(S::GitErrorFailed, error = e)),
        }
    }

    fn git_dir_of(&self, tab: TabId) -> Option<FsPath> {
        let t = self.tabs.iter().find(|t| t.id == tab)?;
        t.git.as_ref().and_then(|g| g.root.clone()).or_else(|| t.view_loc.to_windows())
    }

    pub(super) fn submit_git_auth(&mut self, f: Form, b: usize) -> Option<Form> {
        if let (Kind::GitAuth { dir }, 0) = (f.kind, b) {
            self.launch(Launch::Terminal { dirs: vec![dir], admin: false });
        }
        None
    }
}

fn done_text(op: &GitOp) -> String {
    match op {
        GitOp::Fetch => S::GitDoneFetch.t().to_owned(),
        GitOp::Pull => S::GitDonePull.t().to_owned(),
        GitOp::Push => S::GitDonePush.t().to_owned(),
        GitOp::Sync => S::GitDoneSync.t().to_owned(),
        GitOp::Init => S::GitDoneInit.t().to_owned(),
        GitOp::Clone { url } => crate::tr!(S::GitDoneClone, name = crate::git::repo_name(url).unwrap_or_default()),
        GitOp::Switch(b) => crate::tr!(S::GitDoneSwitch, branch = b),
        GitOp::CreateBranch { name, .. } => crate::tr!(S::GitBranchCreated, branch = name),
        GitOp::DeleteBranch { name, .. } => crate::tr!(S::GitBranchDeleted, branch = name),
        GitOp::OpenRemote => String::new(),
    }
}

#[cfg(test)]
#[path = "../../test/ui/git.rs"]
mod tests;
