use super::archive::Kind;
use super::form::{Control, Form};
use super::App;
use crate::hash::FxHashSet;
use crate::i18n::{P, S};
use crate::listing::attr;
use crate::location::Location;
use crate::name;

const FIND: usize = 0;
const REPLACE: usize = 1;
const CASE: usize = 2;
const NUMBERING: usize = 3;
const START: usize = 4;
const PREVIEW: usize = 5;
const PREVIEW_ROWS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numbering {
    None,
    After,
    Before,
}

const NUMBERINGS: [Numbering; 3] = [Numbering::None, Numbering::After, Numbering::Before];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub find: String,
    pub replace: String,
    pub case: bool,
    pub numbering: Numbering,
    pub first: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Empty(usize),
    Invalid(usize),
    Twice(usize),
    Taken(usize),
}

#[derive(Clone, Debug)]
pub(super) struct Item {
    dir: Location,
    name: Vec<u8>,
    folder: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Batch {
    items: Vec<Item>,
    taken: FxHashSet<(String, Vec<u8>)>,
    windows: bool,
}

fn split(name: &[u8], folder: bool) -> (&[u8], &[u8]) {
    let ext = if folder { 0 } else { name::ext_len(name) };
    if ext == 0 || ext + 1 >= name.len() {
        (name, b"")
    } else {
        name.split_at(name.len() - ext - 1)
    }
}

fn folded_match(hay: &str, needle: &[char]) -> Option<usize> {
    let mut got: Vec<char> = Vec::with_capacity(needle.len());
    for (at, c) in hay.char_indices() {
        got.extend(c.to_lowercase());
        if got.len() >= needle.len() {
            return (got == needle).then_some(at + c.len_utf8());
        }
    }
    None
}

fn replace_text(s: &str, find: &str, with: &str, case: bool) -> String {
    if case {
        return s.replace(find, with);
    }
    let needle: Vec<char> = find.chars().flat_map(char::to_lowercase).collect();
    let (mut out, mut at) = (String::with_capacity(s.len()), 0);
    while at < s.len() {
        match folded_match(&s[at..], &needle) {
            Some(len) => {
                out.push_str(with);
                at += len;
            }
            None => {
                let c = s[at..].chars().next().unwrap_or_default();
                out.push(c);
                at += c.len_utf8().max(1);
            }
        }
    }
    out
}

fn replace_bytes(s: &[u8], find: &[u8], with: &[u8], case: bool) -> Vec<u8> {
    let (mut out, mut at) = (Vec::with_capacity(s.len()), 0);
    while at < s.len() {
        let hit = s[at..].get(..find.len()).is_some_and(|w| if case { w == find } else { w.eq_ignore_ascii_case(find) });
        if hit {
            out.extend_from_slice(with);
            at += find.len();
        } else {
            out.push(s[at]);
            at += 1;
        }
    }
    out
}

fn replaced(stem: &[u8], spec: &Spec) -> Vec<u8> {
    if spec.find.is_empty() {
        return stem.to_vec();
    }
    match std::str::from_utf8(stem) {
        Ok(s) => replace_text(s, &spec.find, &spec.replace, spec.case).into_bytes(),
        Err(_) => replace_bytes(stem, spec.find.as_bytes(), spec.replace.as_bytes(), spec.case),
    }
}

pub fn new_names(items: &[(&[u8], bool)], spec: &Spec) -> Vec<Vec<u8>> {
    let last = spec.first.saturating_add(items.len().saturating_sub(1) as u64);
    let width = last.to_string().len();
    items
        .iter()
        .enumerate()
        .map(|(k, &(old, folder))| {
            let (stem, ext) = split(old, folder);
            let stem = replaced(stem, spec);
            let number = format!("{:0width$}", spec.first.saturating_add(k as u64)).into_bytes();
            let joined = |a: &[u8], b: &[u8]| if a.is_empty() || b.is_empty() { [a, b].concat() } else { [a, b" ", b].concat() };
            let stem = match spec.numbering {
                Numbering::None => stem,
                Numbering::After => joined(&stem, &number),
                Numbering::Before => joined(&number, &stem),
            };
            [&stem[..], ext].concat()
        })
        .collect()
}

pub fn fold(name: &[u8], windows: bool) -> Vec<u8> {
    match (windows, std::str::from_utf8(name)) {
        (true, Ok(s)) => s.to_lowercase().into_bytes(),
        (true, Err(_)) => name.to_ascii_lowercase(),
        (false, _) => name.to_vec(),
    }
}

pub fn problem(old: &[Vec<u8>], new: &[Vec<u8>], windows: bool, taken: impl Fn(usize, &[u8]) -> bool) -> Option<Problem> {
    let mut seen = FxHashSet::default();
    for (k, n) in new.iter().enumerate() {
        if n.is_empty() || n.trim_ascii().is_empty() {
            return Some(Problem::Empty(k));
        }
        let bad = n == b"." || n == b".." || n.iter().any(|&b| b == b'/' || b == 0) || (windows && crate::limits::bad_name(n).is_some());
        if bad {
            return Some(Problem::Invalid(k));
        }
        if !seen.insert(fold(n, windows)) {
            return Some(Problem::Twice(k));
        }
        if fold(n, windows) != fold(&old[k], windows) && taken(k, &fold(n, windows)) {
            return Some(Problem::Taken(k));
        }
    }
    None
}

pub fn order(old: &[Vec<u8>], new: &[Vec<u8>], moving: Vec<usize>) -> Option<Vec<usize>> {
    let mut left = moving;
    let mut done = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let free = left.iter().position(|&k| !left.iter().any(|&j| j != k && old[j] == new[k]))?;
        done.push(left.remove(free));
    }
    Some(done)
}

impl Batch {
    fn spec(f: &Form) -> Spec {
        let first = f.text(START).trim().parse().unwrap_or(1);
        Spec { find: f.text(FIND).to_owned(), replace: f.text(REPLACE).to_owned(), case: f.checked(CASE), numbering: NUMBERINGS[f.choice(NUMBERING).min(2)], first }
    }

    fn old(&self) -> Vec<Vec<u8>> {
        self.items.iter().map(|i| fold(&i.name, self.windows)).collect()
    }

    fn plan(&self, spec: &Spec) -> (Vec<Vec<u8>>, Result<Vec<usize>, Problem>) {
        let items: Vec<(&[u8], bool)> = self.items.iter().map(|i| (i.name.as_slice(), i.folder)).collect();
        let names = new_names(&items, spec);
        let old: Vec<Vec<u8>> = self.items.iter().map(|i| i.name.clone()).collect();
        let taken = |k: usize, n: &[u8]| self.taken.contains(&(self.items[k].dir.display().into_owned(), n.to_vec()));
        if let Some(p) = problem(&old, &names, self.windows, taken) {
            return (names, Err(p));
        }
        let folded_new: Vec<Vec<u8>> = names.iter().zip(&self.items).map(|(n, i)| [i.dir.display().as_bytes(), b"/", &fold(n, self.windows)].concat()).collect();
        let folded_old: Vec<Vec<u8>> = self.old().iter().zip(&self.items).map(|(n, i)| [i.dir.display().as_bytes(), b"/", n].concat()).collect();
        let changed: Vec<usize> = (0..names.len()).filter(|&k| names[k] != self.items[k].name).collect();
        let first = changed.first().copied().unwrap_or(0);
        match order(&folded_old, &folded_new, changed) {
            Some(o) => (names, Ok(o)),
            None => (names, Err(Problem::Taken(first))),
        }
    }
}

fn problem_text(p: Problem, items: &[Item], names: &[Vec<u8>]) -> String {
    let old = |k: usize| name::display(&items[k].name).into_owned();
    let new = |k: usize| name::display(&names[k]).into_owned();
    match p {
        Problem::Empty(k) => crate::tr!(S::BatchRenameEmpty, name = old(k)),
        Problem::Invalid(k) => crate::tr!(S::BatchRenameInvalid, name = old(k), new = new(k)),
        Problem::Twice(k) => crate::tr!(S::BatchRenameTwice, new = new(k)),
        Problem::Taken(k) => crate::tr!(S::BatchRenameTaken, new = new(k)),
    }
}

impl App {
    pub fn batch_rename_dialog(&mut self) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let l = v.listing();
        let live = |i: u32| l.entries[i as usize].attrs & attr::DELETED == 0;
        let picked: Vec<u32> = v.rows().iter().copied().filter(|&i| v.is_selected(i) && live(i)).collect();
        let kind = t.view_loc.path().kind();
        if picked.is_empty() || matches!(kind, crate::path::PathKind::Computer | crate::path::PathKind::RecycleBin) || self.archive_view().is_some() {
            return;
        }
        let windows = picked.first().is_some_and(|&i| !matches!(t.entry_dir(i), Location::Wsl { .. }) && !self.caps.unix_paths);
        let items: Vec<Item> = picked.iter().map(|&i| Item { dir: t.entry_dir(i), name: l.name(i).to_vec(), folder: l.entries[i as usize].is_dir() }).collect();
        let taken = (0..l.len() as u32).filter(|&i| live(i) && !v.is_selected(i)).map(|i| (t.entry_dir(i).display().into_owned(), fold(l.name(i), windows))).collect();
        let batch = Batch { items, taken, windows };
        let numbering = vec![S::BatchRenameNumberNone.t().to_owned(), S::BatchRenameNumberAfter.t().to_owned(), S::BatchRenameNumberBefore.t().to_owned()];
        let mut controls = vec![
            Control::text(S::BatchRenameFind.t(), "", ""),
            Control::text(S::BatchRenameReplace.t(), "", ""),
            Control::check(S::BatchRenameMatchCase.t(), false),
            Control::choice(S::BatchRenameNumbering.t(), numbering, 0),
            Control::text(S::BatchRenameStart.t(), "1", ""),
        ];
        controls.extend((0..PREVIEW_ROWS + 1).map(|_| Control::line("")));
        let title = crate::trn!(P::BatchRenameTitle, batch.items.len());
        let form = Form::new(Kind::BatchRename(batch), &title, controls, vec![S::BatchRenameRename.t(), S::SettingsPageDialogCancel.t()]);
        self.open_form(form);
        self.batch_rename_changed(FIND);
    }

    pub(super) fn batch_rename_changed(&mut self, _k: usize) {
        let Some(f) = &mut self.dialogs.form else { return };
        let Kind::BatchRename(b) = &f.kind else { return };
        let (names, plan) = b.plan(&Batch::spec(f));
        let shown = b.items.len().min(PREVIEW_ROWS);
        let mut lines: Vec<String> = (0..shown)
            .map(|k| {
                let old = name::display(&b.items[k].name).into_owned();
                if names[k] == b.items[k].name {
                    crate::tr!(S::BatchRenameSame, old = old)
                } else {
                    crate::tr!(S::BatchRenamePreview, old = old, new = name::display(&names[k]))
                }
            })
            .collect();
        if b.items.len() > shown {
            lines.push(crate::trn!(P::BatchRenameMore, b.items.len() - shown));
        }
        let (note, error) = match &plan {
            Err(p) => (problem_text(*p, &b.items, &names), true),
            Ok(o) => (crate::trn!(P::BatchRenameCount, o.len()), false),
        };
        for (k, c) in f.controls.iter_mut().skip(PREVIEW).enumerate() {
            if let Control::Line(text) = c {
                *text = lines.get(k).cloned().unwrap_or_default();
            }
        }
        f.say(note, error);
        self.dirty = true;
    }

    pub(super) fn submit_batch_rename(&mut self, mut f: Form) -> Option<Form> {
        let Kind::BatchRename(b) = &f.kind else { return None };
        let (names, plan) = b.plan(&Batch::spec(&f));
        let order = match plan {
            Ok(o) => o,
            Err(p) => {
                let text = problem_text(p, &b.items, &names);
                f.say(text, true);
                return Some(f);
            }
        };
        let pairs: Vec<(Location, Location)> = order.into_iter().map(|k| (b.items[k].dir.join(&b.items[k].name), b.items[k].dir.join(&names[k]))).collect();
        if !pairs.is_empty() {
            let tab = self.tab().id;
            self.start_renames(tab, pairs);
        }
        None
    }
}

#[cfg(test)]
#[path = "../../test/ui/batch_rename.rs"]
mod tests;
