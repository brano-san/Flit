use super::archive::Kind;
use super::form::{Control, Form};
use super::{App, Effect};
use crate::archive;
use crate::fmt as format;
use crate::i18n::{P, S};
use crate::listing::{attr, Entry};
use crate::location::ProviderId;
use crate::name;
use crate::path::FsPath;
use crate::preview::{self, PreviewRequest, PropGroup};
use crate::provider::{GenToken, OpCtl, OpId, ProviderRequest, Totals};
use bumpalo::Bump;

const MEASURE_IDS: OpId = 1 << 61;

#[derive(Default)]
struct Rows {
    size: Option<usize>,
    on_disk: Option<usize>,
    unpacked: Option<usize>,
    contains: Option<usize>,
    accessed: Option<usize>,
    file_system: Option<usize>,
}

pub(super) struct Measuring {
    pub id: OpId,
    pub ctl: OpCtl,
    packed: bool,
    files: u64,
    folders: u64,
    rows: Rows,
    props: u64,
}

pub(super) fn measured_items(paths: &[FsPath], entries: &[Entry], archive_root: bool) -> Vec<(FsPath, Option<u64>)> {
    let mut items: Vec<(FsPath, Option<u64>)> = paths.iter().zip(entries).map(|(p, e)| (p.clone(), (!e.is_dir()).then_some(e.size))).collect();
    if archive_root {
        items.push((paths[0].clone(), None));
    }
    items
}

pub(super) fn contains_text(files: u64, folders: u64) -> String {
    format!("{}, {}", crate::trn!(P::DetailsCountFiles, files), crate::trn!(P::DetailsCountFolders, folders))
}

pub(super) fn attributes_text(attrs: u32) -> String {
    let names = [(attr::READONLY, S::PropertiesReadOnly), (attr::HIDDEN, S::PropertiesHidden), (attr::SYSTEM, S::PropertiesSystem), (attr::LINK, S::PropertiesLink)];
    names.iter().filter(|(bit, _)| attrs & bit != 0).map(|(_, s)| s.t()).collect::<Vec<_>>().join(", ")
}

impl App {
    fn bytes_text(&self, bytes: u64) -> String {
        let mut size = String::new();
        format::size_in(&mut size, bytes, self.style.decimal);
        crate::trn!(P::PropertiesBytes, bytes, size = size)
    }

    fn date_text(&self, ticks: i64) -> String {
        let mut s = String::new();
        format::datetime_in(&mut s, ticks, self.utc_offset_min, &self.style);
        s
    }

    pub fn properties_dialog(&mut self) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let items: Vec<u32> = v.selected().collect();
        if items.is_empty() {
            return self.properties();
        }
        let paths = match self.item_paths(&items) {
            Ok(p) => p,
            Err(e) => return self.status(e),
        };
        let l = v.listing();
        let entries: Vec<Entry> = items.iter().map(|&i| l.entries[i as usize]).collect();
        let packed = self.archive_view().is_some();
        let folders = entries.iter().filter(|e| e.is_dir()).count() as u64;
        let files = entries.len() as u64 - folders;
        let archive_root = !packed && files == 1 && folders == 0 && archive::format_of(l.name(items[0])).is_some();
        let location = paths[0].parent().map(|p| p.display().into_owned()).unwrap_or_default();
        let counting = S::PropertiesCounting.t();
        let mut controls = Vec::new();
        let mut rows = Rows::default();
        let add = |c: &mut Vec<Control>, label: S, value: &str| {
            c.push(Control::value(label.t(), value));
            Some(c.len() - 1)
        };
        let title = match items.as_slice() {
            &[i] => {
                let arena = Bump::new();
                let shown = name::display(l.name(i)).into_owned();
                add(&mut controls, S::PropertiesName, &shown);
                add(&mut controls, S::PropertiesType, self.type_text(&arena, l, i));
                crate::tr!(S::PropertiesTitle, name = shown)
            }
            _ => {
                add(&mut controls, S::PropertiesSelected, &contains_text(files, folders));
                S::PropertiesTitleMany.t().to_owned()
            }
        };
        add(&mut controls, S::PropertiesLocation, &location);
        if !packed {
            rows.file_system = add(&mut controls, S::PropertiesFileSystem, counting);
        }
        let known = (folders == 0).then(|| entries.iter().map(|e| e.size).sum::<u64>());
        rows.size = add(&mut controls, S::PropertiesSize, &known.map_or_else(|| counting.to_owned(), |b| self.bytes_text(b)));
        if !packed {
            rows.on_disk = add(&mut controls, S::PropertiesOnDisk, counting);
        }
        if archive_root {
            rows.unpacked = add(&mut controls, S::PropertiesUnpacked, counting);
        }
        if folders > 0 || archive_root {
            rows.contains = add(&mut controls, S::PropertiesContains, counting);
        }
        if let [e] = entries.as_slice() {
            if e.created > 0 {
                add(&mut controls, S::PropertiesCreated, &self.date_text(e.created));
            }
            if e.modified > 0 {
                add(&mut controls, S::PropertiesModified, &self.date_text(e.modified));
            }
            if !packed {
                rows.accessed = add(&mut controls, S::PropertiesAccessed, counting);
            }
            let attrs = attributes_text(e.attrs);
            if !attrs.is_empty() {
                add(&mut controls, S::PropertiesAttributes, &attrs);
            }
        }
        let id = MEASURE_IDS + self.archive.take_id();
        let ctl = OpCtl::new(None);
        let tab = self.tab().id;
        let req = ProviderRequest::Measure { tab, id, items: measured_items(&paths, &entries, archive_root), ctl: ctl.clone() };
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req });
        let props = if packed || paths.len() != 1 { 0 } else { self.gen() };
        if props != 0 {
            let cancel = GenToken::default();
            cancel.set(props);
            let req = PreviewRequest { gen: props, cancel, path: paths[0].clone(), kind: preview::Kind::Other, size: 0, details: true, px: 0 };
            self.effects.push(Effect::Preview(req));
        }
        let (files, folders) = if paths.len() > 1 { (files, folders) } else { (0, 0) };
        let mut f = Form::new(Kind::Properties(Measuring { id, ctl, packed, files, folders, rows, props }), &title, controls, vec![S::ArchiveClose.t()]);
        f.say(counting, false);
        self.open_form(f);
    }

    pub(super) fn measured(&mut self, id: OpId, totals: Totals, file_system: Option<String>, done: bool) {
        let Some(mut f) = self.jobs.form.take() else { return };
        let Kind::Properties(m) = &f.kind else {
            self.jobs.form = Some(f);
            return;
        };
        if m.id != id {
            self.jobs.form = Some(f);
            return;
        }
        let size = if m.packed { totals.size + totals.unpacked } else { totals.size };
        let mut values: Vec<(Option<usize>, String)> = vec![
            (m.rows.size, self.bytes_text(size)),
            (m.rows.on_disk, self.bytes_text(totals.on_disk)),
            (m.rows.unpacked, self.bytes_text(totals.unpacked)),
            (m.rows.contains, contains_text(totals.files + m.files, totals.folders + m.folders)),
        ];
        if done {
            values.push((m.rows.file_system, file_system.unwrap_or_else(|| "—".into())));
        }
        if totals.accessed > 0 || done {
            values.push((m.rows.accessed, self.date_text(totals.accessed)));
        }
        for (row, text) in values {
            if let Some(Control::Value { value, .. }) = row.and_then(|k| f.controls.get_mut(k)) {
                *value = text;
            }
        }
        if done {
            f.say(String::new(), false);
        }
        self.jobs.form = Some(f);
        self.dirty = true;
    }

    pub(super) fn properties_props(&mut self, gen: u64, groups: &[PropGroup]) -> bool {
        let Some(f) = &mut self.jobs.form else { return false };
        let Kind::Properties(m) = &f.kind else { return false };
        if m.props != gen {
            return false;
        }
        for (k, v) in groups.iter().flat_map(|g| &g.rows) {
            f.controls.push(Control::value(k, v));
        }
        self.dirty = true;
        true
    }
}

#[cfg(test)]
#[path = "../../test/ui/properties.rs"]
mod tests;
