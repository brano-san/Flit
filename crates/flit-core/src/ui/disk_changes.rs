use super::archive::Kind;
use super::form::{Control, Form};
use super::{App, Effect};
use crate::disk_changes::{Error, Filter, HOUR, Reply, Report, Request};
use crate::i18n::{self, S};
use crate::path::{FsPath, PathKind};
use crate::provider::OpCtl;
use std::fmt::Write;

const SHOWN: usize = 250;

pub(super) enum DiskForm {
    Options { root: FsPath, output: FsPath },
    Pending { id: u64, ctl: OpCtl, root: FsPath, output: FsPath, filter: Filter, elevated: bool },
    Denied { root: FsPath, output: FsPath, filter: Filter },
    Report(Report),
    Export { id: u64, ctl: OpCtl },
}

impl DiskForm {
    pub(super) fn cancel(&self) {
        if let Self::Pending { ctl, .. } | Self::Export { ctl, .. } = self {
            ctl.cancel();
        }
    }
}

fn open(kind: DiskForm, controls: Vec<Control>, buttons: Vec<&'static str>) -> Form {
    Form::new(Kind::DiskChanges(kind), S::DiskChangesTitle.t(), controls, buttons)
}

impl App {
    pub(super) fn disk_changes(&mut self) {
        let path = self.current_location().to_windows().filter(|p| p.kind() == PathKind::Drive && !self.caps.unix_paths);
        let Some(path) = path else {
            return self.status(S::DiskChangesLocalOnly.t());
        };
        let root = path.root_path();
        let output = path.join(b"disk-changes.csv");
        let controls = vec![
            Control::line(&root.display()),
            Control::text(S::DiskChangesHours.t(), "24", ""),
            Control::text(S::DiskChangesNameFilter.t(), "", ""),
            Control::check(S::DiskChangesCreated.t(), true),
            Control::check(S::DiskChangesDeleted.t(), true),
            Control::check(S::DiskChangesRenamed.t(), true),
        ];
        self.open_form(open(DiskForm::Options { root, output }, controls, vec![S::DiskChangesRead.t(), S::WidgetsCancel.t()]));
    }

    fn read_disk_changes(&mut self, root: FsPath, output: FsPath, filter: Filter, elevated: bool) {
        self.disk_changes_id += 1;
        let id = self.disk_changes_id;
        let ctl = OpCtl::default();
        let request = Request::Read { root: root.clone(), filter: filter.clone(), elevated, ctl: ctl.clone() };
        self.effects.push(Effect::DiskChanges { id, request });
        self.open_form(open(DiskForm::Pending { id, ctl, root, output, filter, elevated }, vec![Control::paragraph(S::DiskChangesReading.t().to_owned())], vec![S::WidgetsCancel.t()]));
    }

    pub(super) fn submit_disk_changes(&mut self, mut form: Form) -> Option<Form> {
        let Kind::DiskChanges(kind) = &form.kind else {
            return None;
        };
        match kind {
            DiskForm::Options { root, output } => {
                let hours = form.text(1).trim().parse::<i64>().ok().filter(|h| (1..=8760).contains(h));
                let name = form.text(2).trim();
                let (created, deleted, renamed) = (form.checked(3), form.checked(4), form.checked(5));
                let Some(hours) = hours.filter(|_| name.len() <= 512 && (created || deleted || renamed)) else {
                    form.say(S::DiskChangesFilterInvalid.t(), true);
                    return Some(form);
                };
                let until = crate::fmt::now_ticks();
                let filter = Filter { since: until - HOUR * hours, until, name: name.to_owned(), created, deleted, renamed };
                self.read_disk_changes(root.clone(), output.clone(), filter, false);
            }
            DiskForm::Denied { root, output, filter } => self.read_disk_changes(root.clone(), output.clone(), filter.clone(), true),
            DiskForm::Report(report) => {
                let path = self.caps.native_path(form.text(0).trim()).filter(|p| p.kind() == PathKind::Drive && p.file_name().to_ascii_lowercase().ends_with(b".csv"));
                let Some(path) = path else {
                    form.say(S::DiskChangesCsvInvalid.t(), true);
                    return Some(form);
                };
                self.disk_changes_id += 1;
                let id = self.disk_changes_id;
                let ctl = OpCtl::default();
                self.effects.push(Effect::DiskChanges { id, request: Request::Export { path, report: report.clone(), ctl: ctl.clone() } });
                self.open_form(open(DiskForm::Export { id, ctl }, vec![Control::paragraph(S::DiskChangesExporting.t().to_owned())], vec![S::WidgetsCancel.t()]));
            }
            _ => {}
        }
        None
    }

    pub fn on_disk_changes(&mut self, id: u64, reply: Reply) {
        let Some(form) = self.dialogs.form.take() else {
            return;
        };
        match (&form.kind, reply) {
            (Kind::DiskChanges(DiskForm::Pending { id: expected, root, output, filter, elevated, .. }), Reply::Read(result)) if *expected == id => match result {
                Err(Error::Denied) if !elevated => {
                    let controls = vec![Control::paragraph(S::DiskChangesDenied.t().to_owned())];
                    let buttons = vec![S::DiskChangesAdmin.t(), S::ArchiveClose.t()];
                    let mut ask = open(DiskForm::Denied { root: root.clone(), output: output.clone(), filter: filter.clone() }, controls, buttons);
                    ask.focus = ask.controls.len() + 1;
                    self.open_form(ask);
                }
                Err(error) => self.disk_status(error),
                Ok(report) => self.disk_report(report, output),
            },
            (Kind::DiskChanges(DiskForm::Export { id: expected, .. }), Reply::Exported(result)) if *expected == id => match result {
                Ok(path) => self.status(crate::tr!(S::DiskChangesExported, path = path.display())),
                Err(error) => self.disk_status(error),
            },
            _ => self.dialogs.form = Some(form),
        }
        self.layout();
        self.dirty = true;
    }

    fn disk_status(&mut self, error: Error) {
        match error {
            Error::Denied => self.status(S::DiskChangesDenied.t()),
            Error::Cancelled => self.status(S::DiskChangesCancelled.t()),
            Error::Failed(e) => self.status(crate::tr!(S::DiskChangesFailed, error = i18n::localize(&e))),
        }
    }

    fn disk_report(&mut self, report: Report, output: &FsPath) {
        let mut controls = vec![Control::text(S::DiskChangesCsv.t(), &output.display(), S::ArchiveExtractToHint.t())];
        if report.truncated {
            controls.push(Control::paragraph(S::DiskChangesTruncated.t().to_owned()));
        }
        let shown = report.changes.len().min(SHOWN);
        controls.push(Control::paragraph(crate::tr!(S::DiskChangesCount, shown = shown, total = report.changes.len())));
        controls.extend(report.changes.iter().take(SHOWN).map(|c| {
            let mut line = String::new();
            crate::fmt::datetime(&mut line, c.at, self.utc_offset_min);
            let _ = write!(line, "   {}   {}", c.kind.key().t(), c.name);
            Control::line(&line)
        }));
        self.open_form(open(DiskForm::Report(report), controls, vec![S::DiskChangesExport.t(), S::ArchiveClose.t()]));
    }
}

#[cfg(test)]
#[path = "../../test/ui/disk_changes.rs"]
mod tests;
