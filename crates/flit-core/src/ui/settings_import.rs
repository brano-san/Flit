use super::archive::Kind;
use super::form::{Control, Form};
use super::settings::{self, Doc, Value, SETTINGS};
use super::{App, Effect, SettingsCmd};
use crate::i18n::{self, S};
use crate::tags::{self, Tag, TagRequest, MAX_TAGS};

#[derive(Clone, Debug, PartialEq)]
pub struct ImportPlan {
    before: Vec<Value>,
    values: Vec<(usize, Value)>,
    tags_before: Vec<Tag>,
    tags_after: Vec<Tag>,
    problems: Vec<String>,
}

pub(super) struct AppliedImport {
    before: Vec<Value>,
    after: Vec<Value>,
    tags_before: Vec<Tag>,
    tags_after: Vec<Tag>,
}

fn merge_tags(current: &[Tag], imported: &[Tag]) -> Vec<Tag> {
    let mut result = current.to_vec();
    for item in imported {
        if let Some(k) = tags::find(&result, &item.name) {
            result[k].color = item.color;
        } else if let Some(name) = tags::validate_name(&result, &item.name, None).ok().filter(|_| result.len() < MAX_TAGS) {
            result.push(Tag { name, color: item.color });
        }
    }
    result
}

impl ImportPlan {
    pub fn backup(&self, unknown: &serde_json::Map<String, serde_json::Value>) -> String {
        settings::render(&self.before, unknown, Some(&self.tags_before))
    }

    pub fn current(&self, app: &App) -> bool {
        self.before == app.setting_values() && self.tags_before == app.tag_defs()
    }

    fn changes(&self) -> impl Iterator<Item = (usize, &Value)> {
        self.values.iter().filter_map(|(i, new)| (&self.before[*i] != new).then_some((*i, new)))
    }

    fn controls(&self) -> Vec<Control> {
        let mut controls = Vec::new();
        let programs: Vec<_> = self.changes().filter_map(|(i, _)| {
            let key = SETTINGS[i].id;
            (key == "menu.custom").then_some(key)
        }).collect();
        if !programs.is_empty() {
            controls.push(Control::paragraph(crate::tr!(S::SettingsPageImportPrograms, keys = programs.join(", "))));
        }
        for (i, new) in self.changes() {
            let setting = &SETTINGS[i];
            controls.push(Control::line(setting.id));
            controls.push(diff_line(setting.to_json(&self.before[i]), setting.to_json(new)));
        }
        for (k, tag) in self.tags_after.iter().enumerate() {
            if self.tags_before.get(k) != Some(tag) {
                controls.push(Control::line(&tag.name));
                let old = self.tags_before.get(k).map_or_else(|| S::SettingsPageGuideEmpty.t().to_owned(), color);
                controls.push(diff_line(old, color(tag)));
            }
        }
        controls.extend(self.problems.iter().map(|p| Control::paragraph(i18n::localize(p).into_owned())));
        controls
    }
}

fn diff_line(before: impl std::fmt::Display, after: impl std::fmt::Display) -> Control {
    Control::paragraph(crate::tr!(S::SettingsPageImportChange, before = before, after = after))
}

fn color(tag: &Tag) -> String {
    format!("#{:06X}", tag.color & 0xFF_FFFF)
}

impl App {
    pub fn preview_settings_import(&mut self, doc: Doc) {
        if doc.broken {
            return self.report_problems(&doc.problems);
        }
        if doc.newer() {
            return self.status(S::SettingsPageImportNewer.t());
        }
        let plan = ImportPlan {
            before: self.setting_values(),
            values: doc.values,
            tags_before: self.tag_defs().to_vec(),
            tags_after: merge_tags(self.tag_defs(), &doc.tags),
            problems: doc.problems,
        };
        if plan.changes().next().is_none() && plan.tags_before == plan.tags_after {
            if plan.problems.is_empty() {
                return self.status(S::SettingsPageImportNoChanges.t());
            }
            return self.report_problems(&plan.problems);
        }
        let controls = plan.controls();
        let mut form = Form::new(Kind::SettingsImport(plan), S::SettingsPageImportPreview.t(), controls, vec![S::SettingsPageImportApply.t(), S::WidgetsCancel.t()]);
        form.focus = form.controls.len() + 1;
        self.open_form(form);
    }

    pub(super) fn approve_settings_import(&mut self, plan: ImportPlan) {
        if !plan.current(self) {
            return self.status(S::SettingsPageImportStale.t());
        }
        self.effects.push(Effect::Settings(SettingsCmd::ConfirmImport(Box::new(plan))));
    }

    pub fn apply_confirmed_settings_import(&mut self, plan: ImportPlan, backup: String) {
        if !plan.current(self) {
            return self.status(S::SettingsPageImportStale.t());
        }
        let applied = self.apply_settings(&plan.values);
        let tagged = plan.tags_after.iter().enumerate().filter(|(k, tag)| plan.tags_before.get(*k) != Some(*tag)).count();
        self.import_tag_defs(&plan.tags_before, &plan.tags_after);
        let undo = AppliedImport { before: plan.before, after: self.setting_values(), tags_before: plan.tags_before, tags_after: plan.tags_after };
        let controls = vec![
            Control::paragraph(self.imported(applied, tagged)),
            Control::paragraph(crate::tr!(S::SettingsPageImportBackup, path = backup)),
        ];
        let mut form = Form::new(Kind::SettingsImportDone(undo), S::SettingsPageImportPreview.t(), controls, vec![S::SettingsPageImportUndo.t(), S::ArchiveClose.t()]);
        form.focus = form.controls.len() + 1;
        self.open_form(form);
    }

    pub(super) fn undo_settings_import(&mut self, undo: AppliedImport) {
        let tags_changed = undo.tags_before != undo.tags_after;
        let current_tags = self.tag_defs();
        let assigned = (undo.tags_before.len()..undo.tags_after.len()).any(|k| self.tags().count(k) != 0);
        if self.setting_values() != undo.after || (tags_changed && (assigned || (current_tags != undo.tags_after && current_tags != undo.tags_before))) {
            return self.status(S::SettingsPageImportStale.t());
        }
        self.apply_settings(&undo.before.into_iter().enumerate().collect::<Vec<_>>());
        self.import_tag_defs(&undo.tags_after, &undo.tags_before);
        self.status(S::SettingsPageImportUndone.t());
    }

    fn import_tag_defs(&mut self, before: &[Tag], after: &[Tag]) {
        if before != after {
            self.effects.push(Effect::Tags(TagRequest::ChangeImport { before: before.to_vec(), after: after.to_vec() }));
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/settings_import.rs"]
mod tests;
