use super::App;
use super::archive::Kind;
use super::form::{Control, Form};
use super::ops::Then;
use crate::i18n::S;
use crate::location::ProviderId;
use crate::path::{FsPath, PathKind};
use crate::provider::FileOp;
use crate::undo::Origin;

impl App {
    pub(super) fn dedup_selection(&mut self) {
        let selected = self.selected_items();
        let tab = self.tab();
        let Some(view) = &tab.view else { return self.status(S::DedupSelect.t()) };
        if selected.len() < 2 || tab.view_loc.provider() != ProviderId::Local || tab.view_loc.is_recycle_bin() || self.archive_view().is_some()
            || selected.iter().any(|&n| view.listing().entries[n as usize].is_dir())
        {
            return self.status(S::DedupSelect.t());
        }
        let keep_index = view.focus.filter(|i| selected.contains(i)).unwrap_or(selected[0]);
        let paths = self.item_paths(&selected);
        let Ok(mut paths) = paths else { return self.status(S::DedupSelect.t()) };
        let keep = paths.remove(selected.iter().position(|&i| i == keep_index).unwrap_or(0));
        if !std::iter::once(&keep).chain(&paths).all(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unix)) {
            return self.status(S::DedupSelect.t());
        }
        let mut controls = vec![Control::paragraph(crate::tr!(S::DedupKeep, path = keep.display())), Control::paragraph(S::DedupConsequence.t().to_owned())];
        controls.extend(paths.iter().map(|p| Control::line(&p.display())));
        let kind = Kind::Dedup { keep, candidates: paths };
        let mut form = Form::new(kind, S::DedupTitle.t(), controls, vec![S::DedupReplace.t(), S::WidgetsCancel.t()]);
        form.focus = form.controls.len() + 1;
        self.open_form(form);
    }

    pub(super) fn submit_dedup(&mut self, keep: FsPath, candidates: Vec<FsPath>) {
        self.start_op(FileOp::Dedup { keep, candidates, backups: Vec::new() }, Origin::Fresh, Then::Reload);
    }
}
