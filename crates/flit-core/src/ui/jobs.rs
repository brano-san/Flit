use super::form::Form;
use super::{centered, App, Hit, Route};
use crate::archive::{self, ArchiveOp};
use crate::fmt as format;
use crate::i18n::{P, S};
use crate::input::{Key, Mods};
use crate::limits;
use crate::location::{Location, ProviderId};
use crate::name;
use crate::open::OpenTarget;
use crate::path::FsPath;
use crate::provider::{Conflict, FileOp, OpCtl, OpId, OpProgress, Resolve, Stage};
use crate::render::{Color, Rect};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const PANEL_W: f32 = 420.0;
const PANEL_HEAD: f32 = 40.0;
const ROW_H: f32 = 76.0;
const MAX_ROWS: usize = 5;
const MENU_W: f32 = 180.0;
const MENU_ROW: f32 = 30.0;
const DIALOG_W: f32 = 480.0;
const BUTTON_W: f32 = 104.0;
const BUTTON_H: f32 = 32.0;
const SAMPLES: usize = 60;
const SAMPLE_EVERY: Duration = Duration::from_millis(250);
const MAX_JOBS: usize = 100;

const G_OPS: &str = "\u{E896}";
const G_CLOSE: &str = "\u{E8BB}";
const G_MORE: &str = "\u{E712}";
const G_CHECK: &str = "\u{E73E}";

const CONFLICT_BUTTONS: [S; 4] = [S::UiButtonReplace, S::UiButtonKeepBoth, S::UiButtonSkip, S::UiButtonCancel];
const UNREADABLE_BUTTONS: [S; 2] = [S::UiButtonSkip, S::UiButtonCancel];
const NO_ROOM_BUTTONS: [S; 2] = [S::OpsExtraCopyAnyway, S::UiButtonCancel];
const CONFIRM_BUTTONS: [S; 2] = [S::UiButtonDelete, S::UiButtonCancel];
const RESTORE_ALL_BUTTONS: [S; 2] = [S::UiButtonRestore, S::UiButtonCancel];
const RESTORE_BUTTONS: [S; 2] = [S::UiButtonRestore, S::UiButtonNotNow];
const REPLACE_BUTTONS: [S; 2] = [S::UiButtonReplace, S::UiButtonCancel];

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

pub(super) struct Job {
    pub id: OpId,
    op: FileOp,
    wsl: bool,
    pub dest: Option<Location>,
    pub ctl: OpCtl,
    pub state: JobState,
    pub progress: OpProgress,
    pub asking: bool,
    started: Option<Instant>,
    finished: Option<Instant>,
    mark: Option<(Instant, u64)>,
    speed: f64,
    samples: VecDeque<f32>,
}

impl Job {
    fn title(&self) -> String {
        match (&self.op, self.wsl) {
            (FileOp::Trash(items), true) => crate::tr!(S::UiJobTitleTrashingLinux, what = count(items)),
            (op, _) => job_title(op).unwrap_or_default(),
        }
    }
    fn fraction(&self) -> f32 {
        let p = &self.progress;
        let f = match (self.state == JobState::Done, p.total_bytes, p.total_items) {
            (true, ..) => 1.0,
            (_, b, _) if b > 0 => p.bytes as f64 / b as f64,
            (_, _, n) if n > 0 => p.items as f64 / n as f64,
            _ => 0.0,
        };
        f.clamp(0.0, 1.0) as f32
    }

    fn by_bytes(&self) -> bool {
        self.progress.total_bytes > 0
    }

    fn sample(&mut self, now: Instant) {
        let v = if self.by_bytes() { self.progress.bytes } else { self.progress.items };
        self.started.get_or_insert(now);
        match self.mark {
            Some((t, before)) if now.duration_since(t) >= SAMPLE_EVERY => {
                self.speed = v.saturating_sub(before) as f64 / now.duration_since(t).as_secs_f64();
                self.samples.push_back(self.speed as f32);
                if self.samples.len() > SAMPLES {
                    self.samples.pop_front();
                }
                self.mark = Some((now, v));
            }
            Some(_) => {}
            None => self.mark = Some((now, v)),
        }
    }

    fn status(&self) -> String {
        match &self.state {
            JobState::Running if self.asking => S::UiJobConflict.t().into(),
            JobState::Running if self.progress.stage == Stage::Queued => S::UiJobQueued.t().into(),
            JobState::Running if self.progress.stage == Stage::Preparing => String::new(),
            JobState::Running => format!("{:.0}%", self.fraction() * 100.0),
            JobState::Done => S::UiJobDone.t().into(),
            JobState::Failed(_) => S::UiJobFailed.t().into(),
            JobState::Cancelled => S::UiJobCancelled.t().into(),
        }
    }

    fn detail(&self, decimal: bool) -> String {
        let p = &self.progress;
        let mut s = String::new();
        match &self.state {
            JobState::Running if self.asking => s.push_str(S::UiJobWaitingAnswer.t()),
            JobState::Running if self.ctl.is_cancelled() => s.push_str(S::UiJobCancelling.t()),
            JobState::Running if p.stage == Stage::Queued => s.push_str(S::UiJobWaitingDrive.t()),
            JobState::Running if p.stage == Stage::Preparing => s.push_str(S::UiJobPreparing.t()),
            JobState::Running if self.by_bytes() => {
                let size = |v: u64| {
                    let mut out = String::new();
                    format::size_in(&mut out, v, decimal);
                    out
                };
                crate::tr_into!(&mut s, S::UiJobBytesOf, done = size(p.bytes), total = size(p.total_bytes));
                if self.speed > 0.0 {
                    s.push_str(" · ");
                    crate::tr_into!(&mut s, S::UiJobSpeed, size = size(self.speed as u64));
                }
                s.push_str(" · ");
                crate::trn_into!(&mut s, P::UiJobItemsOf, p.total_items, done = crate::i18n::num(p.items.min(p.total_items)));
            }
            JobState::Running => {
                crate::trn_into!(&mut s, P::UiJobItemsOf, p.total_items, done = crate::i18n::num(p.items.min(p.total_items)));
                if self.speed > 0.0 {
                    s.push_str(" · ");
                    crate::tr_into!(&mut s, S::UiJobItemsSpeed, speed = format_args!("{:.0}", self.speed));
                }
            }
            JobState::Done => match (self.started, self.finished) {
                (Some(a), Some(b)) => {
                    let mut secs = String::new();
                    crate::i18n::decimal(&mut secs, b.duration_since(a).as_secs_f64(), 1);
                    crate::tr_into!(&mut s, S::UiJobCompletedIn, seconds = secs);
                }
                _ => s.push_str(S::UiJobCompleted.t()),
            },
            JobState::Failed(e) => s.push_str(e),
            JobState::Cancelled => s.push_str(S::UiJobCancelledDetail.t()),
        }
        s
    }
}

pub(super) enum Ask {
    Conflict { id: OpId, conflict: Conflict },
    Confirm(FileOp, ProviderId),
    Restore,
    ReplaceExport { path: FsPath, existing: crate::listing::Meta },
}

impl Ask {
    fn unreadable(&self) -> bool {
        matches!(self, Ask::Conflict { conflict, .. } if archive::is_unreadable(conflict) && !limits::too_large(conflict))
    }

    fn too_large(&self) -> bool {
        matches!(self, Ask::Conflict { conflict, .. } if limits::too_large(conflict))
    }

    fn name_conflict(&self) -> bool {
        matches!(self, Ask::Conflict { .. }) && !self.unreadable() && !self.too_large()
    }

    fn first_button(&self) -> usize {
        usize::from(self.name_conflict())
    }
}

#[derive(Default)]
struct Rects {
    button: Rect,
    panel: Rect,
    clear: Rect,
    rows: Vec<Rect>,
    menu: Vec<Rect>,
    dialog: Rect,
    buttons: Vec<Rect>,
    check: Rect,
}

#[derive(Default)]
pub(super) struct Jobs {
    pub list: Vec<Job>,
    pub open: bool,
    pub menu: Option<usize>,
    pub asks: VecDeque<Ask>,
    pub form: Option<Form>,
    pub all: bool,
    pub sel: usize,
    first: usize,
    unseen_failure: bool,
    r: Rects,
}

impl Jobs {
    pub fn running(&self) -> bool {
        self.list.iter().any(|j| j.state == JobState::Running)
    }

    pub fn modal(&self) -> bool {
        !self.asks.is_empty() || self.form.is_some()
    }

    fn visible(&self) -> std::ops::Range<usize> {
        self.first..self.list.len().min(self.first + MAX_ROWS)
    }

    fn row_of(&self, i: usize) -> usize {
        self.list.len() - 1 - i
    }

    fn cancel_rect(row: Rect, s: impl Fn(f32) -> f32) -> Rect {
        Rect::new(row.right() - s(64.0), row.y + s(6.0), s(28.0), s(24.0))
    }

    fn more_rect(row: Rect, s: impl Fn(f32) -> f32) -> Rect {
        Rect::new(row.right() - s(34.0), row.y + s(6.0), s(28.0), s(24.0))
    }
}

fn quoted(p: &FsPath) -> String {
    crate::tr!(S::UiQuoted, name = name::display(p.file_name()))
}

fn count(items: &[FsPath]) -> String {
    match items {
        [one] => quoted(one),
        many => crate::trn!(P::DetailsCountItems, many.len()),
    }
}

fn count_pairs(pairs: &[(FsPath, FsPath)], pick: impl Fn(&(FsPath, FsPath)) -> &FsPath) -> String {
    match pairs {
        [one] => quoted(pick(one)),
        many => crate::trn!(P::DetailsCountItems, many.len()),
    }
}

fn job_title(op: &FileOp) -> Option<String> {
    Some(match op {
        FileOp::Copy { items, dest } => crate::tr!(S::UiJobTitleCopying, what = count(items), dest = quoted(dest)),
        FileOp::Move { items, dest } => crate::tr!(S::UiJobTitleMoving, what = count(items), dest = quoted(dest)),
        FileOp::MoveTo(pairs) => {
            let renames = pairs.iter().all(|(a, b)| a.parent() == b.parent());
            crate::tr!(if renames { S::UiJobTitleRenaming } else { S::UiJobTitleMovingItems }, what = count_pairs(pairs, |p| &p.0))
        }
        FileOp::Paste { dest } => crate::tr!(S::UiJobTitlePasting, dest = quoted(dest)),
        FileOp::Trash(items) => crate::tr!(S::UiJobTitleTrashing, what = count(items)),
        FileOp::Delete(items) => crate::tr!(S::UiJobTitleDeleting, what = count(items)),
        FileOp::Flatten(dirs) => crate::tr!(S::UiJobTitleFlattening, what = count(dirs)),
        FileOp::Restore(pairs) => crate::tr!(S::UiJobTitleRestoring, what = count_pairs(pairs, |p| &p.1)),
        FileOp::Purge(pairs) => crate::tr!(S::UiJobTitlePurging, what = count_pairs(pairs, |p| &p.1)),
        FileOp::EmptyBin => S::UiJobTitleEmptying.t().into(),
        FileOp::Archive(ArchiveOp::Compress { items, archive, .. }) => crate::tr!(S::UiJobTitleCompressing, what = count(items), dest = quoted(archive)),
        FileOp::Archive(ArchiveOp::Extract { archives, .. }) => crate::tr!(S::UiJobTitleExtracting, what = count(archives)),
        FileOp::Archive(ArchiveOp::Open { item }) => crate::tr!(S::UiJobTitleOpening, what = quoted(item)),
        FileOp::InstallFonts(fonts) => crate::tr!(S::OpsExtraInstallingFonts, what = count(fonts)),
        FileOp::Rename { .. } | FileOp::NewFolder { .. } | FileOp::NewFile { .. } | FileOp::NewStream { .. } | FileOp::Link { .. } | FileOp::FolderIcon { .. } | FileOp::LockScreen(_) => return None,
    })
}

fn job_dest(provider: &ProviderId, op: &FileOp) -> Option<Location> {
    let path = match op {
        FileOp::Copy { dest, .. } | FileOp::Move { dest, .. } | FileOp::Paste { dest } => dest.clone(),
        FileOp::MoveTo(pairs) => pairs.first()?.1.parent()?,
        FileOp::Restore(pairs) => pairs.first()?.1.parent()?,
        FileOp::Flatten(dirs) => dirs.first()?.clone(),
        FileOp::Trash(_) if provider == &ProviderId::Local => return Some(Location::recycle_bin()),
        FileOp::Archive(ArchiveOp::Compress { archive, .. }) => archive.parent()?,
        FileOp::Archive(ArchiveOp::Extract { archives, dest, .. }) => dest.clone().or_else(|| archives.first()?.parent())?,
        _ => return None,
    };
    Some(Location::on(provider, path))
}

fn confirm_text(op: &FileOp) -> (&'static str, String) {
    let names = |v: Vec<&FsPath>| match v.as_slice() {
        [one] => crate::tr!(S::UiConfirmDeleteOne, name = quoted(one)),
        many => crate::trn!(P::UiConfirmDeleteMany, many.len()),
    };
    match op {
        FileOp::Trash(items) if items.len() == 1 => (S::UiConfirmTrash.t(), crate::tr!(S::UiConfirmTrashOne, name = quoted(&items[0]))),
        FileOp::Trash(items) => (S::UiConfirmTrash.t(), crate::trn!(P::UiConfirmTrashMany, items.len())),
        FileOp::Delete(items) => (S::UiConfirmDelete.t(), names(items.iter().collect())),
        FileOp::Purge(pairs) => (S::UiConfirmDelete.t(), names(pairs.iter().map(|(_, original)| original).collect())),
        FileOp::Restore(pairs) if pairs.len() == 1 => (S::UiConfirmRestore.t(), crate::tr!(S::UiConfirmRestoreOne, name = quoted(&pairs[0].1))),
        FileOp::Restore(pairs) => (S::UiConfirmRestore.t(), crate::trn!(P::UiConfirmRestoreMany, pairs.len())),
        _ => (S::UiConfirmEmpty.t(), S::UiConfirmEmptyText.t().into()),
    }
}

fn meta_line(label: S, m: &crate::listing::Meta, offset: i32, style: &format::Style) -> String {
    let mut what = String::new();
    if m.attrs & crate::listing::attr::DIR != 0 {
        what.push_str(S::UiConflictFolder.t());
    } else {
        format::size_in(&mut what, m.size, style.decimal);
    }
    if m.modified > 0 {
        let mut date = String::new();
        format::datetime_in(&mut date, m.modified, offset, style);
        crate::tr!(S::UiConflictMetaModified, label = label.t(), what = what, date = date)
    } else {
        crate::tr!(S::UiConflictMeta, label = label.t(), what = what)
    }
}

fn can_delete_instead(ask: &Ask) -> bool {
    matches!(ask, Ask::Confirm(FileOp::Trash(_), _))
}

impl App {
    pub(super) fn track_job(&mut self, id: OpId, provider: &ProviderId, op: &FileOp, ctl: &OpCtl) {
        if job_title(op).is_none() {
            return;
        }
        let job = Job { id, op: op.clone(), wsl: matches!(provider, ProviderId::Wsl(_)), dest: job_dest(provider, op), ctl: ctl.clone(), state: JobState::Running, progress: OpProgress::default(), asking: false, started: None, finished: None, mark: None, speed: 0.0, samples: VecDeque::new() };
        if let Some(k) = self.jobs.list.iter().position(|j| j.state != JobState::Running).filter(|_| self.jobs.list.len() >= MAX_JOBS) {
            self.jobs.list.remove(k);
        }
        self.jobs.list.push(job);
        self.layout();
        self.dirty = true;
    }

    fn job_mut(&mut self, id: OpId) -> Option<&mut Job> {
        self.jobs.list.iter_mut().find(|j| j.id == id)
    }

    pub(super) fn job_progress(&mut self, id: OpId, progress: OpProgress, now: Instant) {
        match self.job_mut(id) {
            Some(j) => {
                j.progress = progress;
                j.sample(now);
            }
            None => self.form_progress(id, &progress),
        }
        self.dirty = true;
    }

    pub(super) fn job_conflict(&mut self, id: OpId, conflict: Conflict) {
        if let Some(j) = self.job_mut(id) {
            j.asking = true;
        }
        self.push_ask(Ask::Conflict { id, conflict });
    }

    fn push_ask(&mut self, ask: Ask) {
        if self.jobs.asks.is_empty() {
            self.jobs.all = false;
            self.jobs.sel = ask.first_button();
        }
        self.jobs.asks.push_back(ask);
        self.layout();
        self.dirty = true;
    }

    pub(super) fn job_done(&mut self, id: OpId, error: Option<&str>, now: Instant) {
        let asked = |a: &Ask| matches!(a, Ask::Conflict { id: k, .. } if *k == id);
        if self.jobs.asks.front().is_some_and(asked) {
            self.jobs.all = false;
            self.jobs.sel = self.jobs.asks.iter().find(|a| !asked(a)).map_or(0, Ask::first_button);
        }
        self.jobs.asks.retain(|a| !asked(a));
        let Some(j) = self.job_mut(id) else { return };
        j.state = match error {
            _ if j.ctl.is_cancelled() => JobState::Cancelled,
            Some(e) => JobState::Failed(e.to_owned()),
            None => JobState::Done,
        };
        j.started.get_or_insert(now);
        j.finished = Some(now);
        j.asking = false;
        let failed = matches!(j.state, JobState::Failed(_));
        self.jobs.unseen_failure |= failed && !self.jobs.open;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn ask_confirm(&mut self, op: FileOp) {
        self.ask_confirm_on(ProviderId::Local, op);
    }

    pub(super) fn ask_confirm_on(&mut self, provider: ProviderId, op: FileOp) {
        self.push_ask(Ask::Confirm(op, provider));
    }

    pub fn ask_replace_export(&mut self, path: FsPath, size: u64, modified: i64) {
        self.push_ask(Ask::ReplaceExport { path, existing: crate::listing::Meta { size, modified, ..Default::default() } });
    }

    pub fn offer_restore(&mut self) {
        self.push_ask(Ask::Restore);
    }

    pub fn jobs_open(&self) -> bool {
        self.jobs.open
    }

    pub fn job_states(&self) -> Vec<(String, JobState, OpProgress)> {
        self.jobs.list.iter().map(|j| (j.title(), j.state.clone(), j.progress)).collect()
    }

    pub fn asking(&self) -> Option<&str> {
        match self.jobs.asks.front() {
            Some(a) if a.unreadable() => Some("unreadable"),
            Some(a) if a.too_large() => Some("too_large"),
            Some(Ask::Conflict { .. }) => Some("conflict"),
            Some(Ask::Confirm(..)) => Some("confirm"),
            Some(Ask::Restore) => Some("restore"),
            Some(Ask::ReplaceExport { .. }) => Some("replace"),
            None => self.jobs.form.as_ref().map(|f| f.kind.name()),
        }
    }

    pub fn toggle_jobs(&mut self) {
        self.jobs.open = !self.jobs.open;
        self.jobs.menu = None;
        if self.jobs.open {
            self.jobs.unseen_failure = false;
            self.jobs.first = 0;
        }
        self.layout();
        self.dirty = true;
    }

    pub fn clear_completed(&mut self) {
        self.jobs.list.retain(|j| j.state == JobState::Running);
        self.jobs.menu = None;
        self.jobs.first = 0;
        self.layout();
        self.dirty = true;
    }

    pub fn cancel_job(&mut self, row: usize) {
        let i = self.jobs.row_of(row);
        if let Some(j) = self.jobs.list.get(i).filter(|j| j.state == JobState::Running) {
            j.ctl.cancel();
            self.dirty = true;
        }
    }

    fn job_menu(&self, row: usize) -> Vec<S> {
        let Some(j) = self.jobs.list.get(self.jobs.row_of(row)) else { return Vec::new() };
        let mut items = Vec::new();
        if j.dest.is_some() {
            items.push(S::UiJobOpenDestination);
        }
        items.push(if j.state == JobState::Running { S::UiButtonCancel } else { S::UiJobRemove });
        items
    }

    fn job_menu_items(&self, row: usize) -> Vec<&'static str> {
        self.job_menu(row).into_iter().map(S::t).collect()
    }

    fn run_job_menu(&mut self, k: usize, now: Instant) {
        let Some(row) = self.jobs.menu.take() else { return };
        let item = self.job_menu(row).get(k).copied();
        let i = self.jobs.row_of(row);
        match item {
            Some(S::UiJobOpenDestination) => {
                if let Some(folder) = self.jobs.list.get(i).and_then(|j| j.dest.clone()) {
                    self.jobs.open = false;
                    self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
                }
            }
            Some(S::UiButtonCancel) => self.cancel_job(row),
            Some(_) => {
                self.jobs.list.remove(i);
                self.jobs.first = self.jobs.first.min(self.jobs.list.len().saturating_sub(MAX_ROWS));
            }
            None => {}
        }
        self.layout();
        self.dirty = true;
    }

    fn answer(&mut self, button: usize) {
        let Some(ask) = self.jobs.asks.pop_front() else { return };
        let (two, too_large) = (!ask.name_conflict(), ask.too_large());
        match ask {
            Ask::Conflict { id, .. } => {
                let choice = if two { [Some(Resolve::Skip), None][button.min(1)] } else { [Some(Resolve::Replace), Some(Resolve::KeepBoth), Some(Resolve::Skip), None][button.min(3)] };
                if let Some(ctl) = self.op_ctl(id) {
                    match choice {
                        Some(c) => ctl.answer(c, self.jobs.all && !too_large),
                        None => ctl.cancel(),
                    }
                }
                if let Some(j) = self.job_mut(id) {
                    j.asking = false;
                }
            }
            Ask::Confirm(FileOp::Trash(items), provider) if button == 0 && self.jobs.all => self.start_confirmed(provider, FileOp::Delete(items)),
            Ask::Confirm(op, provider) if button == 0 => self.start_confirmed(provider, op),
            Ask::Restore if button == 0 => self.effects.push(super::Effect::RestoreSession),
            Ask::ReplaceExport { path, .. } if button == 0 => {
                let text = self.export_text();
                self.effects.push(super::Effect::Settings(super::SettingsCmd::Export { path, text, replace: true }));
            }
            Ask::Confirm(..) | Ask::Restore | Ask::ReplaceExport { .. } => {}
        }
        self.jobs.all = false;
        self.jobs.sel = self.jobs.asks.front().map_or(0, Ask::first_button);
        self.layout();
        self.dirty = true;
    }

    pub(super) fn dialog_buttons(&self) -> Vec<&'static str> {
        let keys: &[S] = match self.jobs.asks.front() {
            Some(Ask::Conflict { conflict, .. }) if limits::is_short_of_space(conflict) => &NO_ROOM_BUTTONS,
            Some(a) if a.unreadable() || a.too_large() => &UNREADABLE_BUTTONS,
            Some(Ask::Conflict { .. }) => &CONFLICT_BUTTONS,
            Some(Ask::Confirm(FileOp::Restore(_), _)) => &RESTORE_ALL_BUTTONS,
            Some(Ask::Confirm(..)) => &CONFIRM_BUTTONS,
            Some(Ask::Restore) => &RESTORE_BUTTONS,
            Some(Ask::ReplaceExport { .. }) => &REPLACE_BUTTONS,
            None => &[],
        };
        keys.iter().map(|k| k.t()).collect()
    }

    pub(super) fn jobs_key(&mut self, key: Key, m: Mods) -> bool {
        if !self.jobs.asks.is_empty() {
            let n = self.dialog_buttons().len();
            let conflict = self.jobs.asks.front().is_some_and(Ask::name_conflict);
            let plain = !m.ctrl && !m.alt;
            match key {
                Key::Escape => self.answer(n - 1),
                Key::Enter | Key::Space => self.answer(self.jobs.sel),
                Key::Left => self.jobs.sel = (self.jobs.sel + n - 1) % n,
                Key::Right | Key::Tab => self.jobs.sel = (self.jobs.sel + 1) % n,
                Key::Char('R') if conflict && plain => self.answer(0),
                Key::Char('K') if conflict && plain => self.answer(1),
                Key::Char('S') if conflict && plain => self.answer(2),
                Key::Char('A') if conflict && plain => self.jobs.all = !self.jobs.all,
                _ => {}
            }
            self.dirty = true;
            return true;
        }
        if self.form_key(key, m) {
            return true;
        }
        if self.jobs.open && key == Key::Escape {
            self.jobs.open = false;
            self.jobs.menu = None;
            self.dirty = true;
            return true;
        }
        false
    }

    pub(super) fn jobs_wheel(&mut self, dy: f32) -> bool {
        if !self.jobs.open || !matches!(self.hover, Hit::OpsPanel | Hit::OpsRow(_) | Hit::OpsCancel(_) | Hit::OpsMore(_)) {
            return false;
        }
        let max = self.jobs.list.len().saturating_sub(MAX_ROWS) as isize;
        self.jobs.first = (self.jobs.first as isize - dy.signum() as isize).clamp(0, max) as usize;
        self.jobs.menu = None;
        self.layout();
        self.dirty = true;
        true
    }

    pub(super) fn ops_button_visible(&self) -> bool {
        self.ops_button_always || self.jobs.running() || self.jobs.open || self.jobs.unseen_failure
    }

    pub(super) fn layout_jobs(&mut self) {
        self.layout_form();
        let labels = self.dialog_buttons().len();
        let (w, h, sc) = (self.size.0, self.size.1, self.scale);
        let s = |v: f32| (v * sc).round();
        let button = self.layout.ops_button;
        let j = &mut self.jobs;
        j.r = Rects { button, ..Default::default() };
        if j.open && button.w > 0.0 {
            let pw = s(PANEL_W).min(w - s(16.0));
            let x = (button.right() - pw).max(s(8.0));
            let y = self.layout.toolbar.bottom() + s(2.0);
            let rows = j.visible().len().max(1);
            j.r.panel = Rect::new(x, y, pw, s(PANEL_HEAD) + rows as f32 * s(ROW_H) + s(6.0));
            j.r.clear = Rect::new(j.r.panel.right() - s(140.0), y + s(6.0), s(130.0), s(28.0));
            j.r.rows = j.visible().map(|k| Rect::new(x + s(6.0), y + s(PANEL_HEAD) + (k - j.first) as f32 * s(ROW_H), pw - s(12.0), s(ROW_H))).collect();
            if let Some(row) = j.menu.filter(|r| j.visible().contains(r)) {
                let more = Jobs::more_rect(j.r.rows[row - j.first], s);
                let n = self_menu_len(j, row);
                j.r.menu = (0..n).map(|k| Rect::new(more.right() - s(MENU_W), more.bottom() + s(2.0) + k as f32 * s(MENU_ROW), s(MENU_W), s(MENU_ROW))).collect();
            }
        }
        if let Some(ask) = j.asks.front() {
            let checked = (matches!(ask, Ask::Conflict { .. }) && !ask.too_large()) || can_delete_instead(ask);
            let (buttons, dh) = match ask {
                Ask::Conflict { .. } if ask.too_large() => (labels, s(168.0)),
                Ask::Conflict { .. } => (labels, s(236.0)),
                _ if checked => (labels, s(208.0)),
                Ask::Confirm(..) | Ask::Restore | Ask::ReplaceExport { .. } => (labels, s(168.0)),
            };
            let dw = s(DIALOG_W).min(w - s(32.0));
            let d = Rect::new(((w - dw) / 2.0).round(), ((h - dh) / 2.0).round().max(s(40.0)), dw, dh);
            let (bw, gap) = (s(BUTTON_W).min((dw - s(40.0) - s(8.0) * (buttons as f32 - 1.0)) / buttons as f32), s(8.0));
            let right = d.right() - s(20.0);
            j.r.buttons = (0..buttons).map(|k| Rect::new(right - (buttons - k) as f32 * bw - (buttons - 1 - k) as f32 * gap, d.bottom() - s(20.0) - s(BUTTON_H), bw, s(BUTTON_H))).collect();
            j.r.check = if checked { Rect::new(d.x + s(20.0), d.bottom() - s(20.0) - s(BUTTON_H) - s(40.0), dw - s(40.0), s(28.0)) } else { Rect::default() };
            j.r.dialog = d;
        }
    }

    pub(super) fn jobs_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let r = &self.jobs.r;
        if self.jobs.asks.is_empty() && self.jobs.form.is_some() {
            return self.form_hit(x, y);
        }
        if self.jobs.modal() {
            if let Some(k) = r.buttons.iter().position(|b| b.contains(x, y)) {
                return Some(Hit::DialogButton(k));
            }
            if r.check.contains(x, y) {
                return Some(Hit::DialogCheck);
            }
            if self.layout.title.contains(x, y) {
                return Some(self.layout.caption.iter().position(|c| c.contains(x, y)).map_or(Hit::None, Hit::Caption));
            }
            return Some(if r.dialog.contains(x, y) { Hit::Dialog } else { Hit::Modal });
        }
        if self.nav.palette.is_none() && r.button.contains(x, y) {
            return Some(Hit::OpsButton);
        }
        if !self.jobs.open {
            return None;
        }
        if let Some(k) = r.menu.iter().position(|m| m.contains(x, y)) {
            return Some(Hit::OpsMenu(k));
        }
        if !r.panel.contains(x, y) {
            return None;
        }
        if r.clear.contains(x, y) {
            return Some(Hit::OpsClear);
        }
        let s = |v: f32| (v * self.scale).round();
        for (k, &row) in r.rows.iter().enumerate() {
            let i = self.jobs.first + k;
            if Jobs::more_rect(row, s).contains(x, y) {
                return Some(Hit::OpsMore(i));
            }
            let running = self.jobs.list.get(self.jobs.row_of(i)).is_some_and(|j| j.state == JobState::Running);
            if running && Jobs::cancel_rect(row, s).contains(x, y) {
                return Some(Hit::OpsCancel(i));
            }
            if row.contains(x, y) {
                return Some(Hit::OpsRow(i));
            }
        }
        Some(Hit::OpsPanel)
    }

    pub(super) fn jobs_mouse_down(&mut self, h: Hit) -> bool {
        let ours = matches!(h, Hit::Modal | Hit::Dialog | Hit::DialogButton(_) | Hit::DialogCheck | Hit::Form(_) | Hit::OpsButton | Hit::OpsPanel | Hit::OpsRow(_) | Hit::OpsClear | Hit::OpsCancel(_) | Hit::OpsMore(_) | Hit::OpsMenu(_));
        if !ours {
            if self.jobs.open {
                self.jobs.open = false;
                self.jobs.menu = None;
                self.layout();
            }
            return false;
        }
        if self.editing {
            self.cancel_edit();
        }
        if !matches!(h, Hit::OpsMore(_) | Hit::OpsMenu(_)) {
            self.jobs.menu = None;
        }
        self.pressed = h;
        self.dirty = true;
        true
    }

    pub(super) fn jobs_click(&mut self, h: Hit, now: Instant) -> bool {
        match h {
            Hit::OpsButton => self.toggle_jobs(),
            Hit::OpsClear => self.clear_completed(),
            Hit::OpsCancel(row) => self.cancel_job(row),
            Hit::OpsMore(row) => {
                self.jobs.menu = if self.jobs.menu == Some(row) { None } else { Some(row) };
                self.layout();
            }
            Hit::OpsMenu(k) => self.run_job_menu(k, now),
            Hit::DialogButton(k) => self.answer(k),
            Hit::DialogCheck => self.jobs.all = !self.jobs.all,
            Hit::Form(_) => return self.form_up(h),
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub(super) fn draw_ops_button<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.layout.ops_button;
        if r.w <= 0.0 {
            return;
        }
        let (s2, s4) = (self.s(2.0), self.s(4.0));
        if self.hover == Hit::OpsButton || self.jobs.open {
            let c = if self.pressed == Hit::OpsButton || self.jobs.open { t.button_pressed } else { t.button_hover };
            g.dl.round_rect(r.inset(s2, 0.0), c, s4);
        }
        let running: Vec<f32> = self.jobs.list.iter().filter(|j| j.state == JobState::Running).map(Job::fraction).collect();
        let color = if running.is_empty() { t.text } else { t.accent };
        centered(g, r, G_OPS, self.icon_font(12.0), color);
        if !running.is_empty() {
            let f = running.iter().sum::<f32>() / running.len() as f32;
            let bar = Rect::new(r.x + self.s(8.0), r.bottom() - self.s(5.0), r.w - self.s(16.0), s2.max(2.0));
            g.dl.round_rect(bar, t.separator, s2);
            g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * f).max(s2), bar.h), t.accent, s2);
        }
        if self.jobs.unseen_failure {
            let d = self.s(6.0);
            g.dl.round_rect(Rect::new(r.right() - self.s(9.0), r.y + self.s(4.0), d, d), t.error, d / 2.0);
        }
    }

    pub(super) fn draw_jobs<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        if self.jobs.open && !self.search.active {
            self.draw_panel(g, t);
        }
        if self.jobs.form.is_some() {
            self.draw_form(g, t);
        }
        if !self.jobs.asks.is_empty() {
            self.draw_dialog(g, t);
        }
    }

    pub(super) fn shadow(&self) -> Color {
        if self.dark { Color::rgba(0x00000090) } else { Color::rgba(0x00000030) }
    }

    fn draw_panel<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (font, small, bold) = (self.ui_font(13.0), self.ui_font(12.0), FontStyle::new(FontFamily::Ui, 14.0 * self.scale, true));
        let icons = self.icon_font(10.0);
        let (s4, s6, s12) = (self.s(4.0), self.s(6.0), self.s(12.0));
        let s = |v: f32| (v * self.scale).round();
        let p = self.jobs.r.panel;
        g.dl.shadow(Rect::new(p.x, p.y + s4, p.w, p.h), self.shadow(), s6, s12);
        g.dl.round_rect(p, t.bg, s6);
        g.dl.outline(p, t.input_border, s6, 1.0);
        let m = g.metrics(bold);
        let head = (p.y + (s(PANEL_HEAD) + m.ascent - m.descent) / 2.0).round();
        g.text(p.x + s12 + s6, head, S::UiJobOperations.t(), bold, t.text, p.w / 2.0);
        let clear = self.jobs.r.clear;
        let done = self.jobs.list.iter().any(|j| j.state != JobState::Running);
        if self.hover == Hit::OpsClear && done {
            g.dl.round_rect(clear, t.button_hover, s4);
        }
        let label = S::UiJobClearCompleted.t();
        let lw = g.measure(label, small);
        g.text(clear.right() - lw - s(8.0), head, label, small, if done { t.accent } else { t.text_dim.alpha(0x70) }, f32::INFINITY);
        g.dl.set_clip(Some(p));
        if self.jobs.list.is_empty() {
            let baseline = (p.y + s(PANEL_HEAD) + s(ROW_H) / 2.0).round();
            g.text(p.x + s12 + s6, baseline, S::UiJobNone.t(), font, t.text_dim, p.w - 2.0 * s12);
        }
        let rows = self.jobs.r.rows.clone();
        for (k, &row) in rows.iter().enumerate() {
            let i = self.jobs.first + k;
            let Some(j) = self.jobs.list.get(self.jobs.row_of(i)) else { continue };
            if matches!(self.hover, Hit::OpsRow(h) | Hit::OpsCancel(h) | Hit::OpsMore(h) if h == i) || self.jobs.menu == Some(i) {
                g.dl.round_rect(row, t.hover, s4);
            }
            if k > 0 {
                g.dl.rect(Rect::new(row.x + s12, row.y, row.w - 2.0 * s12, 1.0), t.separator);
            }
            let running = j.state == JobState::Running;
            let (more, cancel) = (Jobs::more_rect(row, s), Jobs::cancel_rect(row, s));
            let status = j.status();
            let sw = g.measure(&status, small);
            let title_end = if running { cancel.x } else { more.x } - s(8.0) - sw;
            let mt = g.metrics(font);
            let title_y = (row.y + s(6.0) + (s(24.0) + mt.ascent - mt.descent) / 2.0).round();
            g.text(row.x + s12, title_y, &j.title(), font, t.text, title_end - row.x - s12 - s(8.0));
            let status_color = match j.state {
                JobState::Failed(_) => t.error,
                JobState::Done => t.text_dim,
                _ => t.accent,
            };
            g.text(title_end, title_y, &status, small, status_color, f32::INFINITY);
            for (b, glyph, show, hit) in [(cancel, G_CLOSE, running, Hit::OpsCancel(i)), (more, G_MORE, true, Hit::OpsMore(i))] {
                if !show {
                    continue;
                }
                if self.hover == hit || (hit == Hit::OpsMore(i) && self.jobs.menu == Some(i)) {
                    g.dl.round_rect(b, t.button_hover, s4);
                }
                centered(g, b, glyph, icons, t.text_dim);
            }
            let graph = Rect::new(row.x + s12, row.y + s(30.0), row.w - 2.0 * s12, s(14.0));
            let peak = j.samples.iter().copied().fold(0.0f32, f32::max);
            if peak > 0.0 {
                let bw = graph.w / SAMPLES as f32;
                let x0 = graph.right() - j.samples.len() as f32 * bw;
                for (n, &v) in j.samples.iter().enumerate() {
                    let bh = (graph.h * v / peak).max(1.0);
                    g.dl.rect(Rect::new(x0 + n as f32 * bw, graph.bottom() - bh, bw.max(1.0), bh), t.accent.alpha(0x50));
                }
            }
            let bar = Rect::new(graph.x, graph.bottom() + s(2.0), graph.w, s(4.0));
            g.dl.round_rect(bar, t.separator, s(2.0));
            let fill = match j.state {
                JobState::Failed(_) => t.error,
                JobState::Cancelled => t.text_dim,
                _ => t.accent,
            };
            g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * j.fraction()).max(if j.fraction() > 0.0 { s(4.0) } else { 0.0 }), bar.h), fill, s(2.0));
            let detail = j.detail(self.style.decimal);
            let ms = g.metrics(small);
            let dy = (bar.bottom() + s(4.0) + (s(18.0) + ms.ascent - ms.descent) / 2.0).round();
            let dcolor = if matches!(j.state, JobState::Failed(_)) { t.error } else { t.text_dim };
            g.text(row.x + s12, dy, &detail, small, dcolor, row.w - 2.0 * s12);
        }
        g.dl.set_clip(None);
        if let Some(row) = self.jobs.menu {
            let items = self.job_menu_items(row);
            let menu = self.jobs.r.menu.clone();
            if let (Some(first), Some(last)) = (menu.first(), menu.last()) {
                let bg = Rect::new(first.x, first.y, first.w, last.bottom() - first.y);
                g.dl.shadow(Rect::new(bg.x, bg.y + s4, bg.w, bg.h), self.shadow(), s6, s4 * 3.0);
                g.dl.round_rect(bg, t.bg, s4);
                g.dl.outline(bg, t.input_border, s4, 1.0);
                for (k, (r, label)) in menu.iter().zip(items).enumerate() {
                    if self.hover == Hit::OpsMenu(k) {
                        g.dl.round_rect(r.inset(s(3.0), s(2.0)), t.hover, s4);
                    }
                    let baseline = (r.y + (r.h + g.metrics(font).ascent - g.metrics(font).descent) / 2.0).round();
                    g.text(r.x + s12, baseline, label, font, t.text, r.w - 2.0 * s12);
                }
            }
        }
    }

    pub(super) fn dialog_text(&self) -> Option<(String, Vec<String>)> {
        let offset = self.utc_offset_min;
        Some(match self.jobs.asks.front() {
            Some(Ask::Conflict { conflict: c, .. }) if limits::too_large(c) => {
                let size = |b: u64| {
                    let mut out = String::new();
                    format::size(&mut out, b);
                    out
                };
                let lines = if limits::is_short_of_space(c) {
                    vec![crate::tr!(S::OpsExtraNoRoom, need = size(c.from.size), folder = quoted(&c.target), free = size(c.to.size)), S::OpsExtraNoRoomHint.t().into()]
                } else {
                    vec![crate::tr!(S::OpsExtraOverLimit, name = quoted(&c.source), size = size(c.from.size), limit = size(c.to.size)), S::OpsExtraOverLimitHint.t().into()]
                };
                (S::OpsExtraTooLargeTitle.t().into(), lines)
            }
            Some(Ask::Conflict { conflict: c, .. }) if archive::is_unreadable(c) => {
                let lines = vec![crate::tr!(S::UiConflictUnreadable, name = quoted(&c.source)), S::UiConflictUnreadableHint.t().into()];
                (S::UiConflictUnreadableTitle.t().into(), lines)
            }
            Some(Ask::Conflict { conflict: c, .. }) => {
                let folder = c.target.parent().map(|p| quoted(&p)).unwrap_or_default();
                let both_dirs = c.from.attrs & c.to.attrs & crate::listing::attr::DIR != 0;
                let mut lines = vec![crate::tr!(S::UiConflictExists, name = quoted(&c.target), folder = folder), meta_line(S::UiConflictNew, &c.from, offset, &self.style), meta_line(S::UiConflictExisting, &c.to, offset, &self.style)];
                if both_dirs {
                    lines.push(S::UiConflictMerge.t().into());
                }
                (S::UiConflictTitle.t().into(), lines)
            }
            Some(Ask::Confirm(op, provider)) => {
                let (title, text) = confirm_text(op);
                let note = match op {
                    FileOp::Trash(_) if *provider != ProviderId::Local => S::UiConfirmNoteLinuxTrash,
                    FileOp::Trash(_) => S::UiConfirmNoteBin,
                    FileOp::Restore(_) => S::UiConfirmNoteRestore,
                    _ => S::UiConfirmNoteUndo,
                }
                .t();
                (title.into(), vec![text, note.into()])
            }
            Some(Ask::ReplaceExport { path, existing }) => (crate::tr!(S::UiExportReplaceTitle, name = quoted(path)), vec![crate::tr!(S::UiExportReplace, folder = path.parent().map(|p| quoted(&p)).unwrap_or_default()), meta_line(S::UiConflictExisting, existing, offset, &self.style)]),
            Some(Ask::Restore) => (S::UiSessionRestoreTitle.t().into(), vec![S::UiSessionCrashed.t().into(), S::UiSessionRestore.t().into()]),
            None => return None,
        })
    }

    pub(super) fn dialog_rects(&self) -> (Rect, &[Rect]) {
        (self.jobs.r.dialog, &self.jobs.r.buttons)
    }

    fn draw_dialog<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (font, small, bold) = (self.ui_font(13.0), self.ui_font(12.0), FontStyle::new(FontFamily::Ui, 16.0 * self.scale, true));
        let s = |v: f32| (v * self.scale).round();
        let (s4, s8, s20) = (s(4.0), s(8.0), s(20.0));
        let (w, h) = self.size;
        g.dl.rect(Rect::new(0.0, 0.0, w, h), if self.dark { Color::rgba(0x00000070) } else { Color::rgba(0x00000040) });
        let d = self.jobs.r.dialog;
        g.dl.shadow(Rect::new(d.x, d.y + s8, d.w, d.h), self.shadow(), s8, s(24.0));
        g.dl.round_rect(d, t.bg, s8);
        g.dl.outline(d, t.input_border, s8, 1.0);
        let Some((title, lines)) = self.dialog_text() else { return };
        let mb = g.metrics(bold);
        let mut y = d.y + s20 + mb.ascent;
        g.text(d.x + s20, y.round(), &title, bold, t.text, d.w - 2.0 * s20);
        y += s(12.0);
        let mf = g.metrics(font);
        for (k, line) in lines.iter().enumerate() {
            y += s(22.0);
            g.text(d.x + s20, y.round() - mf.descent, line, if k == 0 { font } else { small }, if k == 0 { t.text } else { t.text_dim }, d.w - 2.0 * s20);
        }
        let check = self.jobs.r.check;
        if check.w > 0.0 {
            let bx = Rect::new(check.x, check.y + (check.h - s(18.0)) / 2.0, s(18.0), s(18.0));
            if self.jobs.all {
                g.dl.round_rect(bx, t.accent, s4);
                centered(g, bx, G_CHECK, self.icon_font(10.0), t.bg);
            } else {
                g.dl.outline(bx, if self.hover == Hit::DialogCheck { t.accent } else { t.text_dim }, s4, s(1.0).max(1.0));
            }
            let baseline = (check.y + (check.h + mf.ascent - mf.descent) / 2.0).round();
            let label = match self.jobs.asks.front() {
                Some(a) if a.unreadable() => S::UiConflictSkipAll,
                Some(Ask::Confirm(..)) => S::UiConfirmDeleteInstead,
                _ => S::UiConflictApplyAll,
            }
            .t();
            g.text(bx.right() + s8, baseline, label, font, t.text, check.w - bx.w - s8);
        }
        let labels = self.dialog_buttons();
        let buttons = self.jobs.r.buttons.clone();
        for (k, (b, label)) in buttons.iter().zip(labels).enumerate() {
            let primary = k == self.jobs.sel;
            let hot = self.hover == Hit::DialogButton(k);
            if primary {
                g.dl.round_rect(*b, if hot { t.accent.alpha(0xD0) } else { t.accent }, s4);
            } else {
                g.dl.round_rect(*b, if hot { t.button_pressed } else { t.button_hover }, s4);
                g.dl.outline(*b, t.input_border, s4, 1.0);
            }
            let lw = g.measure(label, font);
            let baseline = (b.y + (b.h + mf.ascent - mf.descent) / 2.0).round();
            g.text((b.x + (b.w - lw) / 2.0).round(), baseline, label, font, if primary { t.bg } else { t.text }, b.w);
        }
    }
}

fn self_menu_len(j: &Jobs, row: usize) -> usize {
    j.list.get(j.row_of(row)).map_or(0, |job| 1 + usize::from(job.dest.is_some()))
}

#[cfg(test)]
#[path = "../../test/ui/jobs.rs"]
mod tests;
