use super::form::{Control, Form};
use super::ops::{validate_name, Then};
use super::palette::{Palette, Pick};
use super::{App, Effect, Route};
use crate::archive::{self, ArchiveOp, Compression, Defaults, Encoding, Format, Place, Secret};
use crate::digest::{self, HashKind};
use crate::listing::attr;
use crate::location::{Location, ProviderId};
use crate::name;
use crate::path::{FsPath, PathKind};
use crate::provider::{FileOp, OpCtl, OpId, OpProgress, OpReport, ProviderRequest, TabId};
use crate::undo::Origin;
use crate::i18n::{self, Lang, S};
use std::time::Instant;

const HASH_IDS: OpId = 1 << 62;
const VOLUMES: [(&str, u64); 6] = [("", 0), ("10 MB", 10 << 20), ("100 MB", 100 << 20), ("700 MB", 700 << 20), ("4092 MB", 4092 << 20), ("4.7 GB", 4_700_000_000)];
const DICTIONARIES: [(&str, u32); 7] = [("", 0), ("1 MB", 1 << 20), ("4 MB", 4 << 20), ("16 MB", 16 << 20), ("32 MB", 32 << 20), ("64 MB", 64 << 20), ("256 MB", 256 << 20)];
const THREADS: [(&str, u32); 6] = [("", 0), ("1", 1), ("2", 2), ("4", 4), ("8", 8), ("16", 16)];

const C_NAME: usize = 0;
const C_FORMAT: usize = 1;
const C_LEVEL: usize = 2;
const C_VOLUME: usize = 3;
const C_DICTIONARY: usize = 4;
const C_THREADS: usize = 5;
const C_PASSWORD: usize = 6;
const C_NAMES: usize = 7;

const E_DEST: usize = 0;
const E_RECENT: usize = 1;
const E_PASSWORD: usize = 2;
const E_NAMES: usize = 3;
const E_OPEN: usize = 4;

const H_KINDS: usize = 0;
const H_COMPARE: usize = 1;
const H_VALUES: usize = 2;

pub(super) enum Retry {
    Op(FileOp, Then),
    Reload(TabId),
}

struct Run {
    id: OpId,
    ctl: OpCtl,
    other: Option<FsPath>,
}

pub(super) struct Hashing {
    file: FsPath,
    kinds: Vec<HashKind>,
    values: Vec<String>,
    run: Option<Run>,
}

pub(super) enum Kind {
    Compress { items: Vec<FsPath>, dir: FsPath, single: bool },
    Extract { archives: Vec<FsPath>, entries: Vec<FsPath> },
    Password { archive: FsPath, retry: Retry },
    Hashes(Hashing),
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Compress { .. } => "compress",
            Kind::Extract { .. } => "extract",
            Kind::Password { .. } => "password",
            Kind::Hashes(_) => "hashes",
        }
    }
}

pub(super) struct ArchiveUi {
    pub prefs: Defaults,
    next_id: OpId,
}

impl ArchiveUi {
    pub fn new(prefs: Defaults) -> Self {
        Self { prefs: prefs.sanitized(), next_id: 0 }
    }
}

fn labels<T>(items: &[(&str, T)], zero: S) -> Vec<String> {
    items.iter().map(|(l, _)| if l.is_empty() { zero.t().to_owned() } else { crate::fmt::size_label(l) }).collect()
}

impl App {
    pub(super) fn relabel_form(&mut self, from: Lang) {
        let Some(f) = &mut self.jobs.form else { return };
        let moved = |s: &mut String| *s = i18n::relocalize(s, from).into_owned();
        moved(&mut f.title);
        moved(&mut f.note);
        for b in &mut f.buttons {
            *b = i18n::relocalize_static(b, from);
        }
        for (k, c) in f.controls.iter_mut().enumerate() {
            match c {
                Control::Text { label, placeholder, .. } => {
                    moved(label);
                    *placeholder = i18n::relocalize_static(placeholder, from);
                }
                Control::Choice { label, options, .. } => {
                    moved(label);
                    match (&f.kind, k) {
                        (Kind::Compress { .. }, C_VOLUME) => *options = labels(&VOLUMES, S::ArchiveDoNotSplit),
                        (Kind::Compress { .. }, C_DICTIONARY) => *options = labels(&DICTIONARIES, S::ArchiveAutomatic),
                        _ => options.iter_mut().for_each(moved),
                    }
                }
                Control::Check { label, .. } | Control::Value { label, .. } => moved(label),
                Control::Flags { label, items, .. } => {
                    moved(label);
                    items.iter_mut().for_each(|(s, _)| moved(s));
                }
            }
        }
    }
}

fn position<T: PartialEq + Copy>(items: &[(&str, T)], v: T) -> usize {
    items.iter().position(|&(_, x)| x == v).unwrap_or(0)
}

fn levels(f: Format) -> Vec<String> {
    (0..=f.max_level())
        .map(|n| match n {
            0 => crate::tr!(S::ArchiveLevelStore, n = n),
            1 => crate::tr!(S::ArchiveLevelFastest, n = n),
            n if n == f.default_level() => crate::tr!(S::ArchiveLevelNormal, n = n),
            9 => crate::tr!(S::ArchiveLevelUltra, n = n),
            n => n.to_string(),
        })
        .collect()
}

fn ext_for(f: Format, single_file: bool) -> &'static str {
    if f == Format::Gz && !single_file { Format::TarGz.ext() } else { f.ext() }
}

fn format_note(f: Format) -> &'static str {
    match f {
        Format::Zip => S::ArchiveNoteZip.t(),
        Format::SevenZ => S::ArchiveNote7z.t(),
        Format::Tar => S::ArchiveNoteTar.t(),
        _ => S::ArchiveNoteGz.t(),
    }
}

fn without_ext(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    let exts = Format::WRITABLE.iter().map(|f| f.ext()).chain([Format::TarGz.ext()]);
    let cut = exts.filter(|e| lower.ends_with(&format!(".{e}"))).map(|e| e.len() + 1).max().unwrap_or(0);
    &name[..name.len() - cut]
}

fn kept(mut f: Form, note: &str) -> Option<Form> {
    f.say(note, true);
    Some(f)
}

fn names_label(archive: &FsPath) -> String {
    match (archive::chosen_encoding(archive), archive::detected_encoding(archive)) {
        (Some(e), _) => crate::tr!(S::ArchiveNames, encoding = e.short()),
        (None, Some(d)) => crate::tr!(S::ArchiveNames, encoding = crate::tr!(S::ArchiveAutomaticDetected, encoding = d.short())),
        (None, None) => crate::tr!(S::ArchiveNames, encoding = S::ArchiveAutomatic.t()),
    }
}

impl App {
    pub(super) fn archive_view(&self) -> Option<(FsPath, Vec<u8>)> {
        let t = self.tab();
        let path = t.view_loc.to_windows()?;
        let found = archive::split(&path)?;
        let listed = t.view.as_ref().is_none_or(|v| {
            let l = v.listing();
            l.is_empty() || l.entries.iter().any(|e| e.attrs & attr::ARCHIVED != 0)
        });
        listed.then_some(found)
    }

    pub(super) fn selected_archives(&self) -> Vec<FsPath> {
        let Some(v) = &self.tab().view else { return Vec::new() };
        let items: Vec<u32> = self.selected_items().into_iter().filter(|&i| !v.listing().entries[i as usize].is_dir() && archive::format_of(v.listing().name(i)).is_some()).collect();
        self.item_paths(&items).unwrap_or_default()
    }

    fn quick_stem(&self) -> String {
        let t = self.tab();
        let items = self.selected_items();
        match (&t.view, items.as_slice()) {
            (Some(v), &[i]) if v.listing().entries[i as usize].is_dir() => name::display(v.listing().name(i)).into_owned(),
            (Some(v), &[i]) => name::display(archive::stem(v.listing().name(i))).into_owned(),
            _ => Some(name::display(t.view_loc.file_name()).into_owned()).filter(|n| !n.is_empty() && !n.ends_with(':')).unwrap_or_else(|| S::ArchiveArchive.t().to_owned()),
        }
    }

    pub(super) fn quick_name(&self, f: Format) -> String {
        self.free_name(&self.quick_stem(), &format!(".{}", f.ext()))
    }

    fn compress_items(&mut self) -> Option<(Vec<FsPath>, FsPath, bool)> {
        if self.archive_view().is_some() {
            self.status(crate::i18n::S::ErrorsArchiveInside.t());
            return None;
        }
        let items = self.selected_items();
        if items.is_empty() {
            self.status(S::ArchiveSelectToCompress.t());
            return None;
        }
        let single = items.len() == 1 && self.tab().view.as_ref().is_some_and(|v| !v.listing().entries[items[0] as usize].is_dir());
        match (self.item_paths(&items), self.shell_dir()) {
            (Ok(paths), Ok(dir)) => Some((paths, dir, single)),
            (Err(e), _) | (_, Err(e)) => {
                self.status(e);
                None
            }
        }
    }

    pub fn compress_dialog(&mut self) {
        let Some((items, dir, single)) = self.compress_items() else { return };
        let c = self.archive.prefs.compression.clone();
        let name = self.free_name(&self.quick_stem(), &format!(".{}", ext_for(c.format, single)));
        let controls = vec![
            Control::text(S::ArchiveArchiveName.t(), &name, S::ArchiveArchiveNameHint.t()),
            Control::choice(S::ArchiveFormat.t(), Format::WRITABLE.iter().map(|f| f.label().to_owned()).collect(), Format::WRITABLE.iter().position(|&f| f == c.format).unwrap_or(0)),
            Control::choice(S::ArchiveCompressionLevel.t(), levels(c.format), c.level as usize),
            Control::choice(S::ArchiveSplitVolumes.t(), labels(&VOLUMES, S::ArchiveDoNotSplit), position(&VOLUMES, c.volume)),
            Control::choice(S::ArchiveDictionarySize.t(), labels(&DICTIONARIES, S::ArchiveAutomatic), position(&DICTIONARIES, c.dictionary)),
            Control::choice(S::ArchiveCpuThreads.t(), labels(&THREADS, S::ArchiveAutomatic), position(&THREADS, c.threads)),
            Control::secret(S::ArchivePassword.t(), S::ArchivePasswordCompressHint.t()),
            Control::check(S::ArchiveEncryptNames.t(), c.encrypt_names),
        ];
        let mut f = Form::new(Kind::Compress { items, dir, single }, S::ArchiveAddToArchive.t(), controls, vec![S::ArchiveCompress.t(), S::SettingsPageDialogCancel.t()]);
        f.say(format_note(c.format), false);
        self.open_form(f);
    }

    pub fn compress_quick(&mut self, format: Format) {
        let Some((items, dir, _)) = self.compress_items() else { return };
        let c = &self.archive.prefs.compression;
        let same = c.format == format;
        let options = Compression { format, level: if same { c.level } else { format.default_level() }, dictionary: if same { c.dictionary } else { 0 }, threads: c.threads, ..Default::default() };
        let archive = dir.join(self.quick_name(format).as_bytes());
        self.start_op(FileOp::Archive(ArchiveOp::Compress { items, archive, options }), Origin::Fresh, Then::Select);
    }

    fn submit_compress(&mut self, f: Form) -> Option<Form> {
        let Kind::Compress { items, dir, single } = &f.kind else { return None };
        let (items, dir, single) = (items.clone(), dir.clone(), *single);
        let format = Format::WRITABLE[f.choice(C_FORMAT).min(Format::WRITABLE.len() - 1)];
        let mut name = match validate_name(f.text(C_NAME), dir.kind() != PathKind::Unix) {
            Ok(n) => n,
            Err(e) => return kept(f, e),
        };
        if archive::format_of(name.as_bytes()).is_none() {
            name = format!("{name}.{}", ext_for(format, single));
        }
        let password = f.text(C_PASSWORD).to_owned();
        if !password.is_empty() && !format.encrypts() {
            return kept(f, S::ArchivePasswordFormats.t());
        }
        let options = Compression {
            format,
            level: (f.choice(C_LEVEL) as u8).min(format.max_level()),
            volume: VOLUMES[f.choice(C_VOLUME)].1,
            dictionary: DICTIONARIES[f.choice(C_DICTIONARY)].1,
            threads: THREADS[f.choice(C_THREADS)].1,
            encrypt_names: format == Format::SevenZ && !password.is_empty() && f.checked(C_NAMES),
            password: (!password.is_empty()).then_some(Secret(password)),
        };
        self.archive.prefs.compression = Compression { password: None, encrypt_names: f.checked(C_NAMES), ..options.clone() };
        let op = ArchiveOp::Compress { items, archive: dir.join(name.as_bytes()), options };
        self.start_op(FileOp::Archive(op), Origin::Fresh, Then::Select);
        None
    }

    fn compress_changed(&mut self, k: usize) {
        let Some(f) = &mut self.jobs.form else { return };
        let Kind::Compress { single, .. } = f.kind else { return };
        if k != C_FORMAT {
            return;
        }
        let format = Format::WRITABLE[f.choice(C_FORMAT).min(Format::WRITABLE.len() - 1)];
        let c = &self.archive.prefs.compression;
        let level = if c.format == format { c.level } else { format.default_level() };
        if let Some(Control::Choice { options, sel, .. }) = f.controls.get_mut(C_LEVEL) {
            *options = levels(format);
            *sel = (level as usize).min(options.len() - 1);
        }
        let name = format!("{}.{}", without_ext(f.text(C_NAME)), ext_for(format, single));
        f.set_text(C_NAME, &name);
        f.say(format_note(format), false);
    }

    fn extract_sources(&mut self) -> Option<(Vec<FsPath>, Vec<FsPath>)> {
        if let Some((archive, _)) = self.archive_view() {
            let selected = self.selected_items();
            return match self.item_paths(&selected) {
                Ok(entries) => Some((vec![archive], entries)),
                Err(e) => {
                    self.status(e);
                    None
                }
            };
        }
        let archives = self.selected_archives();
        if archives.is_empty() {
            self.status(S::ArchiveSelectToExtract.t());
            return None;
        }
        Some((archives, Vec::new()))
    }

    pub fn extract_dialog(&mut self) {
        let Some((archives, entries)) = self.extract_sources() else { return };
        let Some(parent) = archives[0].parent() else { return };
        let dest = if archives.len() == 1 && entries.is_empty() { parent.join(archive::stem(archives[0].file_name())) } else { parent };
        let p = &self.archive.prefs;
        let recent = std::iter::once(S::ArchiveChooseRecent.t().to_owned()).chain(p.history.iter().cloned()).collect();
        let detected = archive::detected_encoding(&archives[0]).map_or_else(|| S::ArchiveAutomatic.t().to_owned(), |e| crate::tr!(S::ArchiveAutomaticDetected, encoding = e.short()));
        let encodings = std::iter::once(detected).chain(Encoding::ALL.iter().map(|e| e.label().to_owned())).collect();
        let chosen = archive::chosen_encoding(&archives[0]).and_then(|e| Encoding::ALL.iter().position(|&x| x == e)).map_or(0, |i| i + 1);
        let controls = vec![
            Control::text(S::ArchiveExtractTo.t(), &dest.display(), S::ArchiveExtractToHint.t()),
            Control::choice(S::ArchiveRecentFolders.t(), recent, 0),
            Control::secret(S::ArchivePassword.t(), S::ArchivePasswordExtractHint.t()),
            Control::choice(S::ArchiveNamesEncoding.t(), encodings, chosen),
            Control::check(S::ArchiveOpenWhenDone.t(), p.open_after),
        ];
        let title = if entries.is_empty() { S::ArchiveExtractArchive.t() } else { S::ArchiveExtractSelected.t() };
        self.open_form(Form::new(Kind::Extract { archives, entries }, title, controls, vec![S::ArchiveExtract.t(), S::SettingsPageDialogCancel.t()]));
    }

    fn extract_changed(&mut self, k: usize) {
        let Some(f) = &mut self.jobs.form else { return };
        if k == E_RECENT && matches!(f.kind, Kind::Extract { .. }) {
            if let Some(path) = f.choice(E_RECENT).checked_sub(1).and_then(|i| self.archive.prefs.history.get(i)) {
                f.set_text(E_DEST, &path.clone());
            }
        }
    }

    fn submit_extract(&mut self, f: Form) -> Option<Form> {
        let Kind::Extract { archives, entries } = &f.kind else { return None };
        let (archives, entries) = (archives.clone(), entries.clone());
        let Some(dest) = self.caps.native_path(f.text(E_DEST)) else { return kept(f, S::ArchiveEnterDestination.t()) };
        let text = dest.display().into_owned();
        let p = &mut self.archive.prefs;
        p.history.retain(|h| !h.eq_ignore_ascii_case(&text));
        p.history.insert(0, text);
        p.history.truncate(archive::HISTORY);
        p.open_after = f.checked(E_OPEN);
        let password = f.text(E_PASSWORD);
        let encoding = f.choice(E_NAMES).checked_sub(1).map(|i| Encoding::ALL[i]);
        for a in &archives {
            if !password.is_empty() {
                archive::set_password(a, Some(Secret(password.to_owned())));
            }
            archive::choose_encoding(a, encoding);
        }
        let then = if p.open_after { Then::Open(dest.clone()) } else { Then::Select };
        let parent = archives[0].parent();
        let own = parent.as_ref().map(|p| p.join(archive::stem(archives[0].file_name())));
        let place = if archives.len() > 1 { Place::OwnFolder } else { Place::Here };
        let op = match () {
            _ if !entries.is_empty() => FileOp::Copy { items: entries, dest },
            _ if archives.len() == 1 && own.is_some_and(|o| o.as_bytes().eq_ignore_ascii_case(dest.as_bytes())) => FileOp::Archive(ArchiveOp::Extract { archives, dest: parent, place: Place::OwnFolder }),
            _ => FileOp::Archive(ArchiveOp::Extract { archives, dest: Some(dest), place }),
        };
        self.start_op(op, Origin::Fresh, then);
        None
    }

    pub fn extract_with(&mut self, place: Place) {
        let Some((archives, entries)) = self.extract_sources() else { return };
        let op = match self.archive_view() {
            Some(_) if !entries.is_empty() => match archives[0].parent() {
                Some(dest) => FileOp::Copy { items: entries, dest },
                None => return,
            },
            _ => FileOp::Archive(ArchiveOp::Extract { archives, dest: None, place }),
        };
        self.start_op(op, Origin::Fresh, Then::Select);
    }

    pub fn open_as_folder(&mut self, now: Instant) {
        match self.selected_archives().as_slice() {
            [one] => self.navigate(Location::from_local(one.clone()), now),
            _ => self.status(S::ArchiveSelectOneToOpen.t()),
        }
    }

    pub(super) fn open_archived(&mut self, loc: &Location) -> bool {
        if self.archive_view().is_none() {
            return false;
        }
        self.start_op(FileOp::Archive(ArchiveOp::Open { item: loc.path().clone() }), Origin::Fresh, Then::Launch);
        true
    }

    pub(super) fn browses_as_folder(&self, name: &[u8]) -> bool {
        self.archive_view().is_none() && archive::browsable(name).is_some()
    }

    fn password_retry(op: &FileOp, done: &[(FsPath, FsPath)], then: &Then) -> Option<(FsPath, Retry)> {
        let retry = |op: FileOp| Retry::Op(op, then.clone());
        match op {
            FileOp::Archive(ArchiveOp::Extract { archives, dest, place }) => {
                let left: Vec<FsPath> = archives.iter().filter(|a| !done.iter().any(|(from, _)| from == *a)).cloned().collect();
                let first = left.first()?.clone();
                Some((first, retry(FileOp::Archive(ArchiveOp::Extract { archives: left, dest: dest.clone(), place: *place }))))
            }
            FileOp::Copy { items, .. } => archive::split(items.first()?).map(|(a, _)| (a, retry(op.clone()))),
            FileOp::Archive(ArchiveOp::Open { item }) => archive::split(item).map(|(a, _)| (a, retry(op.clone()))),
            _ => None,
        }
    }

    fn ask_password(&mut self, archive: FsPath, retry: Retry) {
        if self.jobs.form.is_some() {
            return;
        }
        let wrong = archive::password(&archive).is_some();
        let label = name::display(archive.file_name()).into_owned();
        let controls = vec![Control::value(S::ArchiveArchive.t(), &label), Control::secret(S::ArchivePassword.t(), S::ArchivePasswordHint.t())];
        let mut f = Form::new(Kind::Password { archive, retry }, S::ArchiveEncryptedTitle.t(), controls, vec![S::ArchiveOk.t(), S::SettingsPageDialogCancel.t()]);
        f.say(if wrong { crate::i18n::S::ErrorsArchiveWrongPassword.t() } else { crate::i18n::S::ErrorsArchivePasswordKept.t() }, wrong);
        self.open_form(f);
    }

    fn submit_password(&mut self, f: Form) -> Option<Form> {
        let password = f.text(1).to_owned();
        let Form { kind: Kind::Password { archive, retry }, .. } = f else { return None };
        archive::set_password(&archive, Some(Secret(password)));
        match retry {
            Retry::Op(op, then) => self.start_op(op, Origin::Fresh, then),
            Retry::Reload(tab) => {
                if let Some(k) = self.tabs.iter().position(|t| t.id == tab) {
                    self.reload(k, Instant::now());
                }
            }
        }
        None
    }

    pub(super) fn archive_then(&mut self, tab: TabId, then: &Then, report: &OpReport, error: Option<&str>, now: Instant) -> bool {
        if error.is_some_and(archive::needs_password) {
            if let Some((archive, retry)) = Self::password_retry(&report.op, &report.items, then) {
                self.ask_password(archive, retry);
                return true;
            }
        }
        match then {
            Then::Open(dir) if error.is_none() => self.dispatch(vec![Route { folder: Location::from_local(dir.clone()), select: None }], self.open_target, now),
            Then::Launch => {
                for (_, to) in &report.items {
                    if archive::browsable(to.file_name()).is_some() {
                        self.navigate(Location::from_local(to.clone()), now);
                    } else {
                        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { tab, path: to.clone() } });
                    }
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn archive_loaded(&mut self, k: usize) {
        let t = &self.tabs[k];
        let locked = t.view.as_ref().and_then(|v| v.snap().error.as_ref()).is_some_and(|e| e.code == archive::PASSWORD_CODE);
        if !locked || k != self.cur {
            return;
        }
        let tab = t.id;
        if let Some((archive, _)) = t.view_loc.to_windows().and_then(|p| archive::split(&p)) {
            self.ask_password(archive, Retry::Reload(tab));
        }
    }

    pub(super) fn sizes_label(&self) -> String {
        match self.archive_view() {
            None => S::ToolbarButtonFolderSizes.t().to_owned(),
            Some((a, _)) if matches!(archive::format_of(a.file_name()), Some(Format::Zip | Format::Tar | Format::TarGz)) => names_label(&a),
            Some(_) => String::new(),
        }
    }

    pub(super) fn sizes_button(&mut self) {
        if self.archive_view().is_some() {
            self.pick_encoding();
        } else {
            self.compute_dir_sizes();
        }
    }

    pub(super) fn unpacked_total(&self) -> Option<u64> {
        let (archive, inner) = self.archive_view()?;
        if !inner.is_empty() || archive::format_of(archive.file_name()) == Some(Format::Gz) {
            return None;
        }
        Some(self.tab().view.as_ref()?.live_size())
    }

    pub fn pick_encoding(&mut self) {
        let archive = match (self.archive_view(), self.selected_archives().as_slice()) {
            (Some((a, _)), _) => a,
            (None, [one]) => one.clone(),
            _ => return self.status(S::ArchivePickEncodingHint.t()),
        };
        let chosen = archive::chosen_encoding(&archive);
        let mark = |on: bool| if on { S::ArchiveCurrent.t().to_owned() } else { String::new() };
        let detected = archive::detected_encoding(&archive).map_or_else(|| S::ArchiveAutomatic.t().to_owned(), |e| crate::tr!(S::ArchiveAutomaticDetectedLong, encoding = e.short()));
        let mut items = vec![(detected, mark(chosen.is_none()))];
        items.extend(Encoding::ALL.iter().map(|&e| (e.label().to_owned(), mark(chosen == Some(e)))));
        self.open_pick(Palette::new(Pick::Encoding(archive), items, ""));
    }

    pub(super) fn set_encoding(&mut self, archive: &FsPath, id: usize, now: Instant) {
        archive::choose_encoding(archive, id.checked_sub(1).and_then(|i| Encoding::ALL.get(i).copied()));
        self.each_pane(|a, k| {
            if a.tabs[k].view_loc.to_windows().is_some_and(|p| archive::splits(&p).iter().any(|(z, _)| z == archive)) {
                a.reload(k, now);
            }
        });
    }

    pub fn hashes_dialog(&mut self) {
        if self.archive_view().is_some() {
            return self.status(crate::i18n::S::ErrorsArchiveInside.t());
        }
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let selected = self.selected_items();
        let pick = v.focus.filter(|f| selected.contains(f)).or(selected.first().copied()).filter(|&i| !v.listing().entries[i as usize].is_dir());
        let Some(i) = pick else { return self.status(S::ArchiveSelectToHash.t()) };
        let file = match self.item_paths(&[i]) {
            Ok(mut p) => p.remove(0),
            Err(e) => return self.status(e),
        };
        let kinds = self.archive.prefs.hashes.clone();
        let flags = HashKind::ALL.iter().map(|k| (k.label().to_owned(), kinds.contains(k))).collect();
        let mut controls = vec![Control::flags(S::ArchiveAlgorithms.t(), flags), Control::text(S::ArchiveCompareWith.t(), "", S::ArchiveCompareHint.t())];
        controls.extend(kinds.iter().map(|k| Control::value(k.label(), "…")));
        let title = crate::tr!(S::ArchiveHashesTitle, name = name::display(file.file_name()));
        let (id, ctl) = self.hash_file(file.clone(), kinds.clone());
        let mut f = Form::new(Kind::Hashes(Hashing { file, kinds, values: Vec::new(), run: Some(Run { id, ctl, other: None }) }), &title, controls, vec![S::ArchiveCompute.t(), S::ArchiveClose.t()]);
        f.progress = Some(0.0);
        f.say(S::ArchiveReading.t(), false);
        self.open_form(f);
    }

    fn hash_file(&mut self, file: FsPath, kinds: Vec<HashKind>) -> (OpId, OpCtl) {
        self.archive.next_id += 1;
        let id = HASH_IDS + self.archive.next_id;
        let ctl = OpCtl::new(None);
        let tab = self.tab().id;
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Hash { tab, id, file, kinds, ctl: ctl.clone() } });
        (id, ctl)
    }

    fn submit_hashes(&mut self, mut f: Form) -> Option<Form> {
        let chosen: Vec<HashKind> = HashKind::ALL.iter().zip(f.flags(H_KINDS)).filter(|(_, on)| *on).map(|(k, _)| *k).collect();
        if chosen.is_empty() {
            return kept(f, S::ArchiveChooseAlgorithm.t());
        }
        self.archive.prefs.hashes = chosen.clone();
        let Kind::Hashes(h) = &f.kind else { return None };
        if h.run.is_some() {
            return Some(f);
        }
        if chosen != h.kinds || h.values.is_empty() {
            let file = h.file.clone();
            let (id, ctl) = self.hash_file(file, chosen.clone());
            f.controls.truncate(H_VALUES);
            f.controls.extend(chosen.iter().map(|k| Control::value(k.label(), "…")));
            if let Kind::Hashes(h) = &mut f.kind {
                (h.kinds, h.values, h.run) = (chosen, Vec::new(), Some(Run { id, ctl, other: None }));
            }
            f.progress = Some(0.0);
            f.say(S::ArchiveReading.t(), false);
            return Some(f);
        }
        self.compare(&mut f);
        Some(f)
    }

    fn compare(&mut self, f: &mut Form) {
        let text = f.text(H_COMPARE).trim().to_owned();
        let Kind::Hashes(h) = &mut f.kind else { return };
        if text.is_empty() {
            return f.say(S::ArchiveClickToCopy.t(), false);
        }
        if let Some(other) = self.caps.native_path(&text) {
            let (id, ctl) = self.hash_file(other.clone(), h.kinds.clone());
            h.run = Some(Run { id, ctl, other: Some(other) });
            f.progress = Some(0.0);
            return f.say(S::ArchiveReadingOther.t(), false);
        }
        let pairs: Vec<(HashKind, String)> = h.kinds.iter().copied().zip(h.values.iter().cloned()).collect();
        match digest::matching(&text, &pairs) {
            Some(k) => f.say(crate::tr!(S::ArchiveHashMatches, kind = k.label()), false),
            None if digest::expected(&text).is_none() => f.say(S::ArchiveEnterHash.t(), true),
            None => f.say(S::ArchiveHashNoMatch.t(), true),
        }
    }

    pub(super) fn form_progress(&mut self, id: OpId, p: &OpProgress) {
        let Some(f) = &mut self.jobs.form else { return };
        if matches!(&f.kind, Kind::Hashes(Hashing { run: Some(r), .. }) if r.id == id) && p.total_bytes > 0 {
            f.progress = Some((p.bytes as f64 / p.total_bytes as f64) as f32);
        }
    }

    pub(super) fn hashed(&mut self, id: OpId, result: Result<Vec<String>, String>) {
        let Some(mut f) = self.jobs.form.take() else { return };
        let Kind::Hashes(h) = &mut f.kind else {
            self.jobs.form = Some(f);
            return;
        };
        let Some(run) = h.run.take_if(|r| r.id == id) else {
            self.jobs.form = Some(f);
            return;
        };
        f.progress = None;
        match (result, run.other) {
            (Err(e), _) => f.say(e, true),
            (Ok(values), None) => {
                for (c, v) in f.controls.iter_mut().skip(H_VALUES).zip(&values) {
                    if let Control::Value { value, .. } = c {
                        value.clone_from(v);
                    }
                }
                h.values = values;
                self.compare(&mut f);
            }
            (Ok(values), Some(other)) => {
                let same = values == h.values;
                let labels: Vec<&str> = h.kinds.iter().map(|k| k.label()).collect();
                let other = name::display(other.file_name()).into_owned();
                if same {
                    f.say(crate::tr!(S::ArchiveIdentical, other = other, kinds = labels.join(", ")), false);
                } else {
                    f.say(crate::tr!(S::ArchiveDiffers, other = other), true);
                }
            }
        }
        self.jobs.form = Some(f);
        self.layout();
        self.dirty = true;
    }

    pub(super) fn form_changed(&mut self, k: usize) {
        self.compress_changed(k);
        self.extract_changed(k);
    }

    pub(super) fn form_submit(&mut self, b: usize) {
        let Some(f) = self.jobs.form.take() else { return };
        if b + 1 == f.buttons.len() {
            return self.form_closed(f);
        }
        let keep = match f.kind {
            Kind::Compress { .. } => self.submit_compress(f),
            Kind::Extract { .. } => self.submit_extract(f),
            Kind::Password { .. } => self.submit_password(f),
            Kind::Hashes(_) => self.submit_hashes(f),
        };
        if self.jobs.form.is_none() {
            self.jobs.form = keep;
        }
    }

    pub(super) fn form_closed(&mut self, f: Form) {
        if let Kind::Hashes(Hashing { run: Some(r), .. }) = &f.kind {
            r.ctl.cancel();
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/archive.rs"]
mod tests;
