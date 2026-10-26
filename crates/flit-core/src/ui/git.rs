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

const COMMIT_COLUMNS: u8 = 0b1110;

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
    pub(super) fn git_refresh(&mut self, k: usize, changed: bool) {
        let t = &self.tabs[k];
        if changed && t.git.as_ref().is_none_or(|g| g.root.is_none()) {
            return;
        }
        let Some(dir) = git_dir(t.location()) else { return };
        let commits = t.columns.git & COMMIT_COLUMNS != 0;
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

    pub fn on_git(&mut self, ev: GitEvent) {
        match ev {
            GitEvent::Status { tab, gen, info } => self.git_status_arrived(tab, gen, info),
            GitEvent::Branches { tab, branches } => {
                if self.tab().id == tab {
                    self.git_branches = branches;
                    let r = self.layout.branch;
                    let menu = self.build_branch_menu();
                    self.flit_menu(r.x, r.y, menu);
                }
            }
            GitEvent::Started { tab, op } => {
                if self.tab().id == tab {
                    self.status(started_text(&op));
                }
            }
            GitEvent::Done { tab, op, error, url } => {
                let here = self.tab().id == tab;
                match (error, url) {
                    (Some(e), _) if here => self.status(crate::tr!(S::GitErrorFailed, error = e)),
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
        let info = info.filter(|g| g.root.is_some());
        if let Some(k) = self.tabs.iter().position(|t| t.id == tab && t.gen == gen) {
            self.tabs[k].git = info;
            if k == self.cur {
                self.layout();
                self.dirty = true;
            }
            return;
        }
        if let Some(k) = self.inactive_of(tab) {
            self.on_other_at(k, |a| {
                if a.tabs[k].gen == gen {
                    a.tabs[k].git = info;
                    a.layout();
                    a.dirty = true;
                }
            });
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
        let need_repo = !matches!(op, GitOp::Init | GitOp::Clone { .. });
        if let Some((tab, dir)) = self.git_target(need_repo) {
            self.effects.push(Effect::Git(GitRequest::Run { tab, dir, op }));
        }
    }

    pub(super) fn git_branches_menu(&mut self) {
        if let Some((tab, dir)) = self.git_target(true) {
            self.effects.push(Effect::Git(GitRequest::Branches { tab, dir }));
        }
    }

    pub(super) fn git_switch(&mut self, k: usize) {
        if let Some(b) = self.git_branches.get(k).cloned() {
            self.git_run(GitOp::Switch(b));
        }
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
        GitOp::Init => S::GitOpInitializing.t().to_owned(),
        GitOp::Clone { url } => crate::tr!(S::GitOpCloning, url = url),
        GitOp::Switch(b) => crate::tr!(S::GitOpSwitching, branch = b),
        GitOp::OpenRemote => String::new(),
    }
}

fn done_text(op: &GitOp) -> String {
    match op {
        GitOp::Fetch => S::GitDoneFetch.t().to_owned(),
        GitOp::Pull => S::GitDonePull.t().to_owned(),
        GitOp::Push => S::GitDonePush.t().to_owned(),
        GitOp::Init => S::GitDoneInit.t().to_owned(),
        GitOp::Clone { url } => crate::tr!(S::GitDoneClone, name = crate::git::repo_name(url).unwrap_or_default()),
        GitOp::Switch(b) => crate::tr!(S::GitDoneSwitch, branch = b),
        GitOp::OpenRemote => String::new(),
    }
}

#[cfg(test)]
#[path = "../../test/ui/git.rs"]
mod tests;
