use super::App;
use super::archive::Kind;
use super::form::{Control, Form};
use super::menu::Menu;
use super::ops::Then;
use crate::i18n::S;
use crate::location::ProviderId;
use crate::path::FsPath;
use crate::provider::FileOp;
use crate::segments::set_stem;
use crate::undo::Origin;

impl App {
    fn segment_source(&self) -> Option<FsPath> {
        let tab = self.tab();
        if tab.view_loc.provider() != ProviderId::Local || tab.view_loc.is_recycle_bin() || self.archive_view().is_some() {
            return None;
        }
        let view = tab.view.as_ref()?;
        let [index]: [u32; 1] = self.selected_items().try_into().ok()?;
        let entry = &view.listing().entries[index as usize];
        if entry.is_dir() {
            return None;
        }
        tab.entry_location(index)?.to_windows()
    }

    pub(super) fn segments_menu(&self, menu: &mut Menu) {
        let Some(item) = self.segment_source() else { return };
        menu.action("file.split", S::SegmentsSplit.t());
        if std::str::from_utf8(item.file_name()).ok().and_then(set_stem).is_some() {
            menu.action("file.join", S::SegmentsJoin.t());
        }
    }

    pub(super) fn segments_dialog(&mut self, split: bool) {
        let Some(source) = self.segment_source() else { return self.status(S::SegmentsInvalidSource.t()) };
        if !split && std::str::from_utf8(source.file_name()).ok().and_then(set_stem).is_none() {
            return self.status(S::SegmentsInvalidSet.t());
        }
        let Some(parent) = source.parent() else { return };
        let cmd = if split { S::SegmentsSplit } else { S::SegmentsJoin };
        let mut controls = vec![Control::text(S::ArchiveExtractTo.t(), &parent.display(), S::ArchiveExtractToHint.t())];
        if split {
            controls.push(Control::text(S::SegmentsSize.t(), "100", S::SegmentsSize.t()));
        }
        self.open_form(Form::new(Kind::Segments { source, split }, cmd.t(), controls, vec![cmd.t(), S::WidgetsCancel.t()]));
    }

    pub(super) fn submit_segments(&mut self, mut f: Form) -> Option<Form> {
        let Kind::Segments { source, split } = &f.kind else { return None };
        let Some(dest) = self.caps.native_path(f.text(0).trim()) else {
            f.say(S::SettingsPageStatusBadFolder.t(), true);
            return Some(f);
        };
        let part_size = if *split {
            let Some(size) = f.text(1).trim().parse::<u64>().ok().and_then(|n| n.checked_mul(1 << 20)).filter(|&n| n > 0) else {
                f.say(S::SegmentsInvalidSize.t(), true);
                return Some(f);
            };
            Some(size)
        } else {
            None
        };
        self.start_op(FileOp::Segments { source: source.clone(), dest, part_size }, Origin::Fresh, Then::Select);
        None
    }
}
