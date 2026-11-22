use super::archive::Kind;
use super::form::{Control, Form};
use super::permissions::{held_rows, Probe};
use super::ops::Then;
use super::{App, Effect};
use crate::archive;
use crate::fmt as format;
use crate::i18n::{P, S};
use crate::listing::{attr, Entry, Listing};
use crate::location::{Location, ProviderId};
use crate::name;
use crate::path::FsPath;
use crate::preview::{self, PreviewRequest, PropGroup};
use crate::path::PathKind;
use crate::provider::{FileOp, GenToken, OpCtl, OpId, ProviderRequest, Totals};
use crate::undo::Origin;
use bumpalo::Bump;

pub(super) const MEASURE_IDS: OpId = 1 << 61;

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
    pub(super) probe: Probe,
    attrs: Option<AttrEditing>,
}

const EDITABLE: [(u32, S); 2] = [(attr::READONLY, S::PropertiesReadOnly), (attr::HIDDEN, S::PropertiesHidden)];

struct AttrEditing {
    items: Vec<FsPath>,
    at: usize,
    offered: Vec<u32>,
    shown: u32,
    unblock_at: Option<usize>,
}

impl AttrEditing {
    fn op(&self, controls: &[Control]) -> Option<FileOp> {
        let flags: Vec<bool> = match controls.get(self.at) {
            Some(Control::Flags { items, .. }) => items.iter().map(|(_, on)| *on).collect(),
            _ => Vec::new(),
        };
        let (mut set, mut clear) = (0, 0);
        for (&bit, on) in self.offered.iter().zip(flags) {
            let had = self.shown & bit != 0;
            if on && !had {
                set |= bit;
            } else if !on && had {
                clear |= bit;
            }
        }
        let unblock = matches!(self.unblock_at.and_then(|k| controls.get(k)), Some(Control::Check { on: true, .. }));
        (set != 0 || clear != 0 || unblock).then(|| FileOp::SetAttributes { items: self.items.clone(), set, clear, unblock })
    }
}

fn editable_attributes(entries: &[Entry]) -> (Vec<u32>, u32) {
    let folders = entries.iter().any(|e| e.is_dir());
    let offered: Vec<u32> = EDITABLE.iter().map(|&(bit, _)| bit).filter(|&bit| bit != attr::READONLY || !folders).collect();
    let shown = entries.iter().fold(attr::READONLY | attr::HIDDEN, |acc, e| acc & e.attrs);
    (offered, shown)
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
    let names = [(attr::READONLY, S::PropertiesReadOnly), (attr::HIDDEN, S::PropertiesHidden), (attr::SYSTEM, S::PropertiesSystem), (attr::LINK, S::PropertiesLink), (attr::COMPRESSED, S::PropertiesCompressed), (attr::ENCRYPTED, S::PropertiesEncrypted)];
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
        self.properties_for(None);
    }

    pub(super) fn properties_for(&mut self, folder_path: Option<FsPath>) {
        self.properties_of(folder_path.map(|p| (p, true)));
    }

    pub(super) fn properties_of(&mut self, place: Option<(FsPath, bool)>) {
        let t = self.tab();
        let v = t.view.as_ref();
        if v.is_none() && !(place.is_some() && self.caps.unix_paths) {
            return;
        }
        let items: Vec<u32> = match (&place, v) {
            (None, Some(v)) => v.selected().collect(),
            _ => Vec::new(),
        };
        if self.shell_bin() || items.is_empty() && !self.caps.unix_paths {
            return self.properties();
        }
        let folder = items.is_empty();
        let (place, place_attrs) = match place {
            Some((p, false)) => (Some(p), 0),
            p => (p.map(|(p, _)| p), attr::DIR),
        };
        let paths = if folder {
            match place.or_else(|| t.view_loc.to_windows()).filter(|p| p.kind() != crate::path::PathKind::Computer) {
                Some(p) => vec![p],
                None => return,
            }
        } else {
            match self.item_paths(&items) {
                Ok(p) => p,
                Err(e) => return self.status(e),
            }
        };
        let empty;
        let l = match v {
            Some(v) => v.listing(),
            None => {
                empty = Listing::default();
                &empty
            }
        };
        let entries: Vec<Entry> = if folder { vec![Entry { attrs: place_attrs, ..Default::default() }] } else { items.iter().map(|&i| l.entries[i as usize]).collect() };
        let packed = self.archive_view().is_some();
        let folders = entries.iter().filter(|e| e.is_dir()).count() as u64;
        let files = entries.len() as u64 - folders;
        let archive_root = !packed && !folder && files == 1 && folders == 0 && archive::format_of(l.name(items[0])).is_some();
        let location = paths[0].parent().map(|p| p.display().into_owned()).unwrap_or_default();
        let counting = S::PropertiesCounting.t();
        let mut controls = Vec::new();
        let mut rows = Rows::default();
        let add = |c: &mut Vec<Control>, label: S, value: &str| {
            c.push(Control::value(label.t(), value));
            Some(c.len() - 1)
        };
        let title = if folder {
            let (arena, file_name) = (Bump::new(), paths[0].file_name());
            let shown = name::display(file_name).into_owned();
            add(&mut controls, S::PropertiesName, &shown);
            add(&mut controls, S::PropertiesType, self.type_of(&arena, place_attrs, &file_name[file_name.len() - name::ext_len(file_name)..]));
            crate::tr!(S::PropertiesTitle, name = shown)
        } else {
            match items.as_slice() {
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
            }
        };
        add(&mut controls, S::PropertiesLocation, &location);
        if !packed {
            rows.file_system = add(&mut controls, S::PropertiesFileSystem, counting);
        }
        let known = (folders == 0 && !folder).then(|| entries.iter().map(|e| e.size).sum::<u64>());
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
        }
        let editing = !self.caps.unix_paths && !packed && !folder && matches!(t.view_loc, Location::Local(_)) && paths.iter().all(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc));
        let attr_editing = editing.then(|| {
            let (offered, shown) = editable_attributes(&entries);
            let names = offered.iter().filter_map(|&bit| EDITABLE.iter().find(|(b, _)| *b == bit)).map(|&(bit, label)| (label.t().to_owned(), shown & bit != 0)).collect();
            controls.push(Control::flags(S::PropertiesAttributes.t(), names));
            AttrEditing { items: paths.clone(), at: controls.len() - 1, offered, shown, unblock_at: None }
        });
        if let [e] = entries.as_slice() {
            let (label, attrs) = if editing { ("", attributes_text(e.attrs & !(attr::READONLY | attr::HIDDEN))) } else { (S::PropertiesAttributes.t(), attributes_text(e.attrs)) };
            if !attrs.is_empty() {
                controls.push(Control::value(label, &attrs));
            }
        }
        let id = MEASURE_IDS + self.archive.take_id();
        let ctl = OpCtl::new(None);
        let tab = self.tab().id;
        let mut items = measured_items(&paths, &entries, archive_root);
        if folder {
            items[0].1 = None;
        }
        let req = ProviderRequest::Measure { tab, id, items, ctl: ctl.clone() };
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req });
        let props = if packed || paths.len() != 1 { 0 } else { self.gen() };
        if props != 0 {
            let cancel = GenToken::default();
            cancel.set(props);
            let req = PreviewRequest { gen: props, cancel, path: paths[0].clone(), kind: preview::Kind::Other, size: 0, details: true, px: 0 };
            self.effects.push(Effect::Preview(req));
        }
        let probe = if self.caps.unix_paths && !packed && paths.iter().all(|p| p.kind() == crate::path::PathKind::Unix) {
            self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Access { tab, id, items: paths.clone() } });
            Probe::Waiting { items: paths.clone(), held: Vec::new() }
        } else {
            Probe::Off
        };
        let (files, folders) = if paths.len() > 1 { (files, folders) } else { (0, 0) };
        let buttons = vec![S::ArchiveClose.t()];
        let mut f = Form::new(Kind::Properties(Measuring { id, ctl, packed, files, folders, rows, props, probe, attrs: attr_editing }), &title, controls, buttons);
        f.say(counting, false);
        self.open_form(f);
    }

    pub(super) fn measured(&mut self, id: OpId, totals: Totals, file_system: Option<String>, done: bool) {
        let Some(mut f) = self.dialogs.form.take() else { return };
        let Kind::Properties(m) = &f.kind else {
            self.dialogs.form = Some(f);
            return;
        };
        if m.id != id {
            self.dialogs.form = Some(f);
            return;
        }
        let size = if m.packed { totals.size + totals.unpacked } else { totals.size };
        let mut values: Vec<(Option<usize>, String)> =
            vec![(m.rows.size, self.bytes_text(size)), (m.rows.on_disk, self.bytes_text(totals.on_disk)), (m.rows.unpacked, self.bytes_text(totals.unpacked)), (m.rows.contains, contains_text(totals.files + m.files, totals.folders + m.folders))];
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
        self.dialogs.form = Some(f);
        self.dirty = true;
    }

    pub(super) fn properties_props(&mut self, gen: u64, groups: &[PropGroup]) -> bool {
        let Some(Form { kind: Kind::Properties(m), controls, focus, .. }) = &mut self.dialogs.form else { return false };
        if m.props != gen {
            return false;
        }
        for g in groups {
            if g.title == S::PropertiesDates.t() {
                let own = [S::PropertiesCreated.t(), S::PropertiesModified.t(), S::PropertiesAccessed.t(), S::PropertiesChanged.t()];
                for row in &g.rows {
                    let known = own.contains(&row.0.as_str()).then(|| controls.iter_mut().find(|c| matches!(c, Control::Value { label, .. } if label == &row.0))).flatten();
                    if let Some(Control::Value { value, .. }) = known {
                        value.clone_from(&row.1);
                    } else {
                        held_rows(&mut m.probe, controls, focus, &g.title, std::slice::from_ref(row));
                    }
                }
                continue;
            }
            match &mut m.attrs {
                Some(a) if g.title == S::PropertiesSecurity.t() && a.unblock_at.is_none() => {
                    controls.extend(g.rows.iter().map(|(k, v)| Control::value(k, v)));
                    controls.push(Control::check(S::PropertiesUnblock.t(), false));
                    a.unblock_at = Some(controls.len() - 1);
                }
                _ => held_rows(&mut m.probe, controls, focus, &g.title, &g.rows),
            }
        }
        self.layout();
        self.dirty = true;
        true
    }

    pub(super) fn properties_edited(f: &Form) -> bool {
        let Kind::Properties(m) = &f.kind else { return false };
        m.attrs.as_ref().is_some_and(|a| a.op(&f.controls).is_some()) || matches!(&m.probe, Probe::Editing(e) if !matches!(e.edit(&f.controls), Ok(None)))
    }

    pub(super) fn submit_properties_form(&mut self, f: Form) -> Option<Form> {
        if let Kind::Properties(Measuring { attrs: Some(a), .. }) = &f.kind {
            if let Some(op) = a.op(&f.controls) {
                self.start_op(op, Origin::Fresh, Then::Nothing);
            }
        }
        self.submit_properties(f)
    }
}

#[cfg(test)]
#[path = "../../test/ui/properties.rs"]
mod tests;
