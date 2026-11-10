use super::archive::Kind;
use super::form::{Control, Form};
use super::jobs::JobState;
use super::{App, Effect};
use crate::i18n::{P, S};
use crate::provider::Launch;
use crate::update::{percent, Release, UpdateEvent, UpdateRequest, CURRENT, RELEASES_URL};


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdatePrefs {
    pub on_start: bool,
    pub skipped: Vec<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self { on_start: true, skipped: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Ask,
    Busy,
    ReadOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Updating {
    pub release: Release,
    pub stage: Stage,
}

fn buttons(stage: Stage) -> Vec<&'static str> {
    match stage {
        Stage::Ask => vec![S::UpdateDialogInstall.t(), S::UpdateDialogSkip.t(), S::UpdateDialogLater.t()],
        Stage::Busy => vec![S::WidgetsCancel.t()],
        Stage::ReadOnly => vec![S::UpdateDialogOpenReleases.t(), S::UpdateDialogLater.t()],
    }
}

pub(super) fn plain(markdown: &str) -> String {
    let line = markdown.trim_end().replace("**", "");
    let heading = line.trim_start_matches('#');
    if heading.len() < line.len() {
        return heading.trim().to_owned();
    }
    match line.trim_start().strip_prefix("* ").or_else(|| line.trim_start().strip_prefix("- ")) {
        Some(item) => format!("• {item}"),
        None => line,
    }
}

fn note_lines(notes: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in notes.lines().map(plain) {
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

impl App {
    pub fn check_updates(&mut self, manual: bool) {
        if manual {
            self.status(S::UpdateStatusChecking.t());
        }
        self.effects.push(Effect::Update(UpdateRequest::Check { manual }));
    }

    pub fn checks_updates_on_start(&self) -> bool {
        self.updates.on_start
    }

    pub fn file_jobs_running(&self) -> usize {
        self.jobs.list.iter().filter(|j| j.state == JobState::Running).count().max(self.ops_in_flight())
    }

    fn busy_update(&mut self) -> Option<(&mut Form, String)> {
        let f = self.jobs.form.as_mut()?;
        let version = match &f.kind {
            Kind::Update(u) if u.stage == Stage::Busy => u.release.version.clone(),
            _ => return None,
        };
        Some((f, version))
    }

    pub fn on_update(&mut self, ev: UpdateEvent) {
        match ev {
            UpdateEvent::Checked { manual, found: Err(error) } if manual => self.status(crate::tr!(S::UpdateStatusCheckFailed, error = error)),
            UpdateEvent::Checked { manual, found: Ok(None) } if manual => self.status(crate::tr!(S::UpdateStatusUpToDate, version = CURRENT)),
            UpdateEvent::Checked { manual, found: Ok(Some(release)) } => self.offer_update(release, manual),
            UpdateEvent::Checked { .. } => {}
            UpdateEvent::Progress { received, total } => {
                if let Some((f, version)) = self.busy_update() {
                    match percent(received, total) {
                        Some(p) => {
                            f.progress = Some(f32::from(p) / 100.0);
                            f.say(crate::tr!(S::UpdateDialogDownloading, version = version, percent = p), false);
                        }
                        None => f.say(crate::tr!(S::UpdateDialogDownloadingPlain, version = version), false),
                    }
                }
            }
            UpdateEvent::Installing => {
                if let Some((f, version)) = self.busy_update() {
                    f.progress = Some(1.0);
                    f.say(crate::tr!(S::UpdateDialogInstalling, version = version), false);
                }
            }
            UpdateEvent::Failed(error) => self.update_stopped(crate::tr!(S::UpdateStatusFailed, error = error), Stage::Ask),
            UpdateEvent::ReadOnly(dir) => self.update_stopped(crate::tr!(S::UpdateDialogReadOnly, dir = dir), Stage::ReadOnly),
        }
        self.layout();
        self.dirty = true;
    }

    fn offer_update(&mut self, release: Release, manual: bool) {
        if !manual && self.updates.skipped.contains(&release.version) {
            return;
        }
        if let Some(f) = &self.jobs.form {
            if !matches!(f.kind, Kind::Update(_)) {
                self.status(crate::tr!(S::UpdateStatusAvailable, version = release.version));
            }
            return;
        }
        let title = crate::tr!(S::UpdateDialogTitle, version = release.version);
        let mut controls = vec![Control::value(S::UpdateDialogInstalled.t(), CURRENT), Control::value(S::UpdateDialogNew.t(), &release.version)];
        controls.extend(note_lines(&release.notes).iter().map(|l| Control::line(l)));
        let mut form = Form::new(Kind::Update(Updating { release, stage: Stage::Ask }), &title, controls, buttons(Stage::Ask));
        form.say(S::UpdateDialogRestart.t(), false);
        self.open_form(form);
    }

    fn update_stopped(&mut self, text: String, stage: Stage) {
        self.status(text.clone());
        if let Some(f) = self.jobs.form.as_mut() {
            let Kind::Update(u) = &mut f.kind else { return };
            u.stage = stage;
            f.buttons = buttons(stage);
            f.focus = f.controls.len();
            f.progress = None;
            f.say(text, true);
        }
    }

    pub(super) fn submit_update(&mut self, mut f: Form, b: usize) -> Option<Form> {
        let Kind::Update(u) = &mut f.kind else { return None };
        match (u.stage, b) {
            (Stage::Ask, 0) => {
                let running = self.file_jobs_running();
                if running > 0 {
                    f.say(crate::trn!(P::UpdateDialogJobs, running), true);
                    return Some(f);
                }
                u.stage = Stage::Busy;
                let (request, text) = (UpdateRequest::Install(u.release.clone()), crate::tr!(S::UpdateDialogDownloadingPlain, version = u.release.version));
                f.buttons = buttons(Stage::Busy);
                f.focus = f.controls.len();
                f.progress = Some(0.0);
                f.say(text, false);
                self.effects.push(Effect::Update(request));
                Some(f)
            }
            (Stage::Ask, 1) => {
                self.updates.skipped = vec![u.release.version.clone()];
                None
            }
            (Stage::ReadOnly, 0) => {
                self.launch(Launch::Run { program: RELEASES_URL.to_owned(), args: String::new(), dir: None });
                None
            }
            _ => Some(f),
        }
    }

    pub(super) fn cancel_update(&mut self, u: &Updating) {
        if u.stage == Stage::Busy {
            self.effects.push(Effect::Update(UpdateRequest::Cancel));
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/update.rs"]
mod tests;
