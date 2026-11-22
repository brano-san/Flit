use super::archive::Kind;
use super::form::{Control, Form};
use super::{App, Column, Effect, Tab};
use crate::fmt as format;
use crate::git::{GitEvent, GitInfo, GitJob, GitOp, GitRequest, GitResult, GitStatus, Sequence, SwitchChoice};
use crate::i18n::S;
use crate::location::{Location, ProviderId};
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, TabId};
use crate::render::Rect;
use std::sync::Arc;
use std::time::Instant;

const COMMIT_COLUMNS: u8 = 0b11110;

fn sequence_labels(sequence: Sequence) -> [S; 2] {
    match sequence {
        Sequence::Merge => [S::GitSequenceMerge, S::GitSequenceAbortMerge],
        Sequence::Rebase => [S::GitSequenceRebase, S::GitSequenceAbortRebase],
        Sequence::CherryPick => [S::GitSequenceCherryPick, S::GitSequenceAbortCherryPick],
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
        self.git = self.git.as_ref().and_then(|g| {
            let root = g.root.as_ref().filter(|r| loc.path().is_inside(r) || loc.path().same(r))?;
            Some(Arc::new(GitInfo {
                root: Some(root.clone()), branch: g.branch.clone(), upstream: g.upstream.clone(),
                ahead: g.ahead, behind: g.behind, insertions: g.insertions, deletions: g.deletions,
                stashes: g.stashes.clone(), ..Default::default()
            }))
        });
    }

    pub(super) fn git_status(&self, name: &[u8]) -> Option<GitStatus> {
        self.git.as_ref()?.status.get(name).copied()
    }
}

impl App {
    pub(super) fn git_sequence(&self) -> Option<Sequence> {
        git_dir(self.tab().location())?;
        self.git_info().filter(|g| g.root.is_some())?.sequence
    }

    pub(super) fn git_bar_labels(&self) -> Option<[String; 4]> {
        let [state, abort] = sequence_labels(self.git_sequence()?);
        Some([state.t().into(), crate::tr!(S::GitSequenceConflicts, n = self.git_info()?.conflicts), abort.t().into(), S::GitSequenceShow.t().into()])
    }

    pub(super) fn layout_git_bar(&mut self) {
        self.layout.git_bar = [Rect::default(); 4];
        if self.git_sequence().is_none() { return; }
        let h = self.cs(28.0);
        let pad = self.cs(4.0);
        let r = self.layout.header;
        if r.w >= self.cs(800.0) {
            let w = r.w / 4.0;
            self.layout.git_bar = std::array::from_fn(|i| Rect::new(r.x + i as f32 * w + pad, r.y, w - 2.0 * pad, h));
            self.layout.header.y += h + pad;
        } else {
            self.layout.git_bar = std::array::from_fn(|i| Rect::new(r.x + pad, r.y + i as f32 * h, (r.w - 2.0 * pad).max(0.0), h));
            self.layout.header.y += 4.0 * h + pad;
        }
    }

    pub(super) fn draw_git_bar<B: crate::text::TextBackend>(&self, g: &mut crate::text::Gfx<B>, t: &crate::theme::Theme) {
        let Some(labels) = self.git_bar_labels() else { return };
        let font = self.content_font(13.0);
        for (i, (r, label)) in self.layout.git_bar.into_iter().zip(labels).enumerate() {
            g.dl.set_clip(Some(r));
            if i >= 2 {
                let hit = if i == 2 { super::Hit::GitAbort } else { super::Hit::GitConflicts };
                let fill = if self.lit(hit) > 0.0 { t.button_hover } else { t.button_pressed };
                g.dl.round_rect(Rect::new(r.x, r.y + self.cs(2.0), r.w, r.h - self.cs(4.0)), fill, self.cs(3.0));
            }
            super::centered(g, r, &label, font, t.text);
            g.dl.set_clip(None);
        }
    }

    pub(super) fn show_git_conflicts(&mut self) {
        if self.git_sequence().is_none() { return; }
        self.close_filter();
        let t = self.tab_mut();
        if let (Some(g), Some(v)) = (&t.git, &mut t.view) {
            let conflicts: Vec<_> = v.rows().iter().copied().filter(|i| g.status.get(v.listing().name(*i)) == Some(&GitStatus::Conflict)).collect();
            v.clear_selection();
            for &i in &conflicts { v.set_selected(i, true); }
            if let Some(&i) = conflicts.first() { v.focus = Some(i); v.anchor = Some(i); }
        }
        self.focus = super::Focus::List;
        self.reveal_focus(true);
        self.dirty = true;
    }

    pub(super) fn ask_git_abort(&mut self) {
        if self.dialogs.form.is_some() { return; }
        let Some(sequence) = self.git_sequence() else { return };
        let Some((tab, dir)) = self.git_target(true) else { return };
        let label = sequence_labels(sequence)[1].t();
        let controls = vec![Control::paragraph(S::GitSequenceConfirm.t().into())];
        let mut f = Form::new(Kind::GitAbort { tab, dir, sequence, pending: false }, label, controls, vec![label, S::WidgetsCancel.t()]);
        f.focus = f.controls.len() + 1;
        self.open_form(f);
    }

    pub(super) fn submit_git_abort(&mut self, mut f: Form) -> Option<Form> {
        if let Kind::GitAbort { tab, dir, sequence, pending } = &mut f.kind {
            if !*pending {
                *pending = true;
                self.effects.push(Effect::Git(GitRequest::Run { tab: *tab, dir: dir.clone(), op: GitOp::Abort(*sequence) }));
                f.say(S::GitSequenceAborting.t(), false);
            }
        }
        Some(f)
    }

    pub(super) fn set_git_integration(&mut self, on: bool) {
        self.prefs.git_integration = on;
        self.each_pane(|a, k| {
            a.tabs.items[k].git = None;
            if let Some(v) = &mut a.tabs.items[k].view {
                v.set_git(None);
            }
            if a.tabs.items[k].view_spec.sort.field == crate::sort::SortField::GitHash {
                a.resort(k);
            }
            if on { a.git_refresh(k, false); }
        });
        self.layout();
        self.dirty = true;
    }

    pub(super) fn git_refresh(&mut self, k: usize, changed: bool) {
        if !self.prefs.git_integration { return; }
        let t = &self.tabs.items[k];
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

    pub(super) fn git_branch_label(&self) -> Option<String> {
        use std::fmt::Write;
        let g = self.git_info().filter(|g| g.root.is_some() && !g.branch.is_empty())?;
        let mut label = g.branch.clone();
        if g.ahead != 0 || g.behind != 0 {
            let _ = write!(label, "  ↑{} ↓{}", g.ahead, g.behind);
        }
        if g.insertions != 0 || g.deletions != 0 {
            let _ = write!(label, "  +{} −{}", g.insertions, g.deletions);
        }
        if !g.stashes.is_empty() {
            label.push_str("  ");
            label.push_str(&crate::trn!(crate::i18n::P::GitStashCount, g.stashes.len()));
        }
        Some(label)
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
                if self.tab().id == tab && self.tab().gen == gen && git_dir(self.tab().location()).as_ref() == Some(&dir) && self.dialogs.form.is_none() {
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
            GitEvent::SwitchBlocked { tab, dir, from, branch } => {
                if self.tab().id == tab && git_dir(self.tab().location()).as_ref() == Some(&dir) {
                    if self.dialogs.form.is_none() {
                        self.ask_git_switch(tab, dir, from, branch);
                    } else {
                        self.status(crate::tr!(S::GitBranchSwitchBlocked, branch = branch));
                    }
                }
            }
            GitEvent::AuthFailed { tab, dir, op, error, github } => {
                let error = crate::redact::passwords(&error).into_owned();
                if self.tab().id == tab {
                    self.open_git_auth(&op, dir, &error, github);
                }
                self.each_pane(|a, k| a.git_refresh(k, true));
            }
            GitEvent::Progress { id, progress, .. } => self.git_job_progress(id, progress),
            GitEvent::IndexLockRemoved { id, error, .. } => {
                self.git_index_lock_removed(id, error.as_deref());
                if error.is_none() { self.each_pane(|a, k| a.git_refresh(k, true)); }
            }
            GitEvent::TrackedDone { tab, id, op, result } => {
                let now = Instant::now();
                let result = match result {
                    GitResult::Failed { error, auth } => GitResult::Failed { error: crate::redact::passwords(&error).into_owned(), auth },
                    result => result,
                };
                let error = match &result { GitResult::Failed { error, .. } => Some(error.as_str()), _ => None };
                self.job_done(id, error, matches!(result, GitResult::Cancelled | GitResult::CancelledWithLock(_)), now);
                match result {
                    GitResult::Success => self.on_git(GitEvent::Done { tab, op, error: None, url: None }),
                    GitResult::Failed { error, auth: Some((dir, github)) } => self.on_git(GitEvent::AuthFailed { tab, dir, op, error, github }),
                    GitResult::Failed { error, auth: None } => self.on_git(GitEvent::Done { tab, op, error: Some(error), url: None }),
                    result @ (GitResult::Cancelled | GitResult::CancelledWithLock(_)) => {
                        if let GitResult::CancelledWithLock(lock) = result { self.git_job_lock(id, lock); }
                        if self.tab().id == tab { self.status(S::GitOpCancelled.t()); }
                        self.each_pane(|a, k| a.git_refresh(k, true));
                    }
                }
            }
            GitEvent::Done { tab, op, error, url } => {
                let error = error.map(|e| crate::redact::passwords(&e).into_owned());
                let here = self.tab().id == tab;
                let handled = self.git_form_done(tab, &op, error.as_deref());
                match (error, url) {
                    (Some(e), _) if here && !handled => self.git_failed(tab, &op, &e),
                    (None, Some(url)) => self.launch(Launch::Run { program: url, args: String::new(), dir: None }),
                    (None, None) if here => self.status(done_text(&op)),
                    _ => {}
                }
                self.each_pane(|a, k| a.git_refresh(k, true));
                if op == GitOp::Init {
                    self.git_refresh(self.tabs.cur, false);
                }
            }
        }
    }

    fn git_status_arrived(&mut self, tab: TabId, gen: u64, info: Option<Arc<GitInfo>>) {
        if !self.prefs.git_integration { return; }
        let info = info.filter(|g| g.root.is_some());
        if let Some(k) = self.tabs.items.iter().position(|t| t.id == tab && t.gen == gen) {
            self.tabs.items[k].git = info;
            if self.tabs.items[k].view_spec.sort.field == crate::sort::SortField::GitHash {
                self.resort(k);
            }
            if k == self.tabs.cur {
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
            self.git_run_in(tab, dir, op);
        }
    }

    fn git_run_in(&mut self, tab: TabId, dir: FsPath, op: GitOp) {
        let request = if op.is_network() {
            let job = GitJob { id: self.allocate_op_id(), ctl: Default::default() };
            let title = match &op {
                GitOp::Clone { url } => crate::tr!(S::GitOpCloningName, name = crate::git::repo_name(url).unwrap_or_default()),
                _ => started_text(&op),
            };
            self.track_git_job(&job, &dir, title);
            GitRequest::RunTracked { tab, dir, op, job }
        } else {
            GitRequest::Run { tab, dir, op }
        };
        self.effects.push(Effect::Git(request));
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

    fn ask_git_switch(&mut self, tab: TabId, dir: FsPath, from: String, branch: String) {
        let controls = vec![
            Control::paragraph(crate::tr!(S::GitBranchSwitchBlocked, branch = branch)),
            Control::choice(S::GitBranchSwitchChoice.t(), [S::GitBranchSwitchMerge, S::GitBranchSwitchStash, S::GitBranchSwitchDiscard].map(|s| s.t().to_owned()).to_vec(), 0),
        ];
        let kind = Kind::GitSwitch { tab, dir, from, branch, confirm_discard: false };
        self.open_form(Form::new(kind, S::GitBranchSwitchTitle.t(), controls, vec![S::GitBranchSwitchContinue.t(), S::WidgetsCancel.t()]));
    }

    pub(super) fn submit_git_switch(&mut self, mut f: Form) -> Option<Form> {
        let selected = f.choice(1);
        let Kind::GitSwitch { tab, dir, from, branch, confirm_discard } = &mut f.kind else { return None };
        let choice = if *confirm_discard {
            SwitchChoice::Discard
        } else {
            match selected {
                0 => SwitchChoice::Merge,
                1 => SwitchChoice::Stash,
                _ => {
                    *confirm_discard = true;
                    f.controls = vec![Control::paragraph(crate::tr!(S::GitBranchSwitchDiscardConfirm, branch = branch))];
                    f.buttons[0] = S::GitBranchSwitchDiscard.t();
                    f.focus = f.controls.len() + 1;
                    return Some(f);
                }
            }
        };
        let op = GitOp::RetrySwitch { from: from.clone(), branch: branch.clone(), choice };
        self.effects.push(Effect::Git(GitRequest::Run { tab: *tab, dir: dir.clone(), op }));
        None
    }

    pub(super) fn flit_stash(&self) -> Option<&crate::git::Stash> {
        let g = self.git_info()?;
        let ours = || g.stashes.iter().filter(|s| s.flit_branch().is_some());
        ours().find(|s| s.flit_branch() == Some(g.branch.as_str())).or_else(|| ours().next())
    }

    pub(super) fn ask_git_drop_stash(&mut self, stash: crate::git::Stash) {
        if self.dialogs.form.is_some() { return; }
        let Some((tab, dir)) = self.git_target(true) else { return };
        let kind = Kind::GitStashDrop { tab, dir, oid: stash.oid };
        let controls = vec![Control::paragraph(crate::tr!(S::GitStashDropConfirm, stash = stash.message))];
        let mut f = Form::new(kind, S::GitStashDrop.t(), controls, vec![S::GitStashDrop.t(), S::WidgetsCancel.t()]);
        f.focus = f.controls.len() + 1;
        self.open_form(f);
    }

    pub(super) fn submit_git_drop_stash(&mut self, f: Form) -> Option<Form> {
        if let Kind::GitStashDrop { tab, dir, oid } = f.kind {
            self.effects.push(Effect::Git(GitRequest::Run { tab, dir, op: GitOp::DropStash(oid) }));
        }
        None
    }

    pub(super) fn ask_git_branch(&mut self) {
        if self.dialogs.form.is_some() { return; }
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
        if self.dialogs.form.is_some() { return; }
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

    fn git_form_done(&mut self, tab: TabId, op: &GitOp, error: Option<&str>) -> bool {
        let Some(mut f) = self.dialogs.form.take() else { return false };
        let handled = match &mut f.kind {
            Kind::GitAbort { tab: t, sequence, pending, .. } if *t == tab && *pending && *op == GitOp::Abort(*sequence) => {
                *pending = false;
                if let Some(e) = error { f.say(e, true); }
                true
            }
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
        if !handled || error.is_some() { self.dialogs.form = Some(f); }
        if handled { self.show_conflict_table(); self.layout(); self.dirty = true; }
        handled
    }

    pub(super) fn ask_clone(&mut self) {
        if let Some((tab, dir)) = self.git_target(false) {
            self.ask_clone_in(tab, dir, "");
        }
    }

    pub(super) fn ask_clone_in(&mut self, tab: TabId, dir: FsPath, url: &str) -> bool {
        if self.dialogs.form.is_some() || git_dir(&Location::from_local(dir.clone())).is_none() {
            return false;
        }
        let controls = vec![Control::text(S::GitCloneUrl.t(), url, S::GitCloneHint.t()), Control::value(S::GitCloneDestination.t(), &dir.display())];
        self.open_form(Form::new(Kind::GitClone { tab, dir }, S::GitCloneTitle.t(), controls, vec![S::GitCloneButton.t(), S::SettingsPageDialogCancel.t()]));
        true
    }

    pub(super) fn submit_clone(&mut self, f: Form) -> Option<Form> {
        let url = f.text(0).trim().to_owned();
        if crate::git::repo_name(&url).is_none() {
            return Some(f);
        }
        if let Kind::GitClone { tab, dir } = f.kind {
            self.git_run_in(tab, dir, GitOp::Clone { url });
        }
        None
    }

    pub(super) fn submit_git_remove_index_lock(&mut self, f: Form) -> Option<Form> {
        if let Kind::GitRemoveIndexLock { id, lock } = f.kind {
            self.effects.push(Effect::Git(GitRequest::RemoveIndexLock { tab: self.tab().id, id, lock }));
        }
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
        GitOp::RetrySwitch { branch, .. } => crate::tr!(S::GitOpSwitching, branch = branch),
        GitOp::PopStash(_) => S::GitStashRestoring.t().into(),
        GitOp::ApplyStash(_) => S::GitStashApplying.t().into(),
        GitOp::DropStash(_) => S::GitStashDropping.t().into(),
        GitOp::CreateBranch { .. } => S::GitBranchCreating.t().into(),
        GitOp::DeleteBranch { .. } => S::GitBranchDeleting.t().into(),
        GitOp::Abort(_) => S::GitSequenceAborting.t().into(),
        GitOp::OpenRemote => String::new(),
    }
}

impl App {
    fn git_failed(&mut self, tab: TabId, op: &GitOp, e: &str) {
        match crate::git::failure(e) {
            crate::git::Failure::Missing => self.status(if self.caps.unix_paths { S::GitErrorInstallLinux } else { S::GitErrorInstallWindows }.t()),
            crate::git::Failure::Auth if self.dialogs.form.is_none() => {
                let Some(dir) = self.git_dir_of(tab) else { return self.status(crate::tr!(S::GitErrorFailed, error = e)) };
                self.open_git_auth(op, dir, e, None);
            }
            _ => self.status(crate::tr!(S::GitErrorFailed, error = e)),
        }
    }

    fn open_git_auth(&mut self, op: &GitOp, dir: FsPath, error: &str, github: Option<crate::git::GithubLogin>) {
        use crate::git::GithubLogin;
        self.status(crate::tr!(S::GitErrorFailed, error = error));
        let Some(command) = crate::git::op_command(op).filter(|_| self.dialogs.form.is_none()) else { return };
        let mut controls = vec![Control::paragraph(crate::tr!(S::GitErrorAuthText, command = command)), Control::line(error)];
        let mut buttons = vec![S::GitErrorAuthOpen.t()];
        let login = match github {
            Some(GithubLogin::Command(argv)) => {
                buttons.push(S::GitErrorGithubLogin.t());
                Some(argv)
            }
            Some(GithubLogin::Install(package)) => {
                controls.push(Control::paragraph(crate::tr!(S::GitErrorGithubInstall, package = package)));
                None
            }
            None => None,
        };
        buttons.push(S::WidgetsCancel.t());
        self.open_form(Form::new(Kind::GitAuth { dir, login }, S::GitErrorAuthTitle.t(), controls, buttons));
    }

    fn git_dir_of(&self, tab: TabId) -> Option<FsPath> {
        let t = self.tabs.items.iter().find(|t| t.id == tab)?;
        t.git.as_ref().and_then(|g| g.root.clone()).or_else(|| t.view_loc.to_windows())
    }

    pub(super) fn submit_git_auth(&mut self, f: Form, b: usize) -> Option<Form> {
        match (f.kind, b) {
            (Kind::GitAuth { dir, .. }, 0) => self.launch(Launch::Terminal { dirs: vec![dir], admin: false }),
            (Kind::GitAuth { dir, login: Some(argv) }, 1) => self.launch(Launch::RunInTerminalCommand { dir, argv }),
            _ => {}
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
        GitOp::RetrySwitch { branch, choice: SwitchChoice::Stash, .. } => crate::tr!(S::GitStashSwitched, branch = branch),
        GitOp::RetrySwitch { branch, .. } => crate::tr!(S::GitDoneSwitch, branch = branch),
        GitOp::PopStash(_) => S::GitStashRestored.t().into(),
        GitOp::ApplyStash(_) => S::GitStashApplied.t().into(),
        GitOp::DropStash(_) => S::GitStashDropped.t().into(),
        GitOp::CreateBranch { name, .. } => crate::tr!(S::GitBranchCreated, branch = name),
        GitOp::DeleteBranch { name, .. } => crate::tr!(S::GitBranchDeleted, branch = name),
        GitOp::Abort(_) => S::GitSequenceAborted.t().into(),
        GitOp::OpenRemote => String::new(),
    }
}

#[cfg(test)]
#[path = "../../test/ui/git.rs"]
mod tests;
