use super::actions::{chord_text, index, Chord, ACTIONS};
use crate::i18n::{self, Lang, S};
use crate::{tr, tr_en};
use super::palette::{Palette, Pick};
use super::settings_page::SettingPick;
use super::App;
use crate::hash::FxHashMap;
use crate::input::{Key, Mods};
use serde_json::{Map, Value as Json};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Keymap {
    custom: Vec<Option<Vec<Chord>>>,
    unknown: Map<String, Json>,
    owners: FxHashMap<Chord, usize>,
    words: [OnceLock<Vec<String>>; 2],
}

impl PartialEq for Keymap {
    fn eq(&self, other: &Self) -> bool {
        self.custom == other.custom && self.unknown == other.unknown
    }
}

impl Eq for Keymap {}

impl Keymap {
    fn new(custom: Vec<Option<Vec<Chord>>>, unknown: Map<String, Json>) -> Self {
        let mut k = Self { custom, unknown, owners: FxHashMap::default(), words: Default::default() };
        k.reindex();
        k
    }

    fn reindex(&mut self) {
        let mut owners = std::mem::take(&mut self.owners);
        owners.clear();
        for i in (0..ACTIONS.len()).filter(|&i| ACTIONS[i].rebindable()) {
            for &c in self.keys(i) {
                owners.entry(c).or_insert(i);
            }
        }
        self.owners = owners;
        self.words = Default::default();
    }

    pub fn words(&self, i: usize) -> &str {
        let l = i18n::lang();
        let all = self.words[l as usize].get_or_init(|| {
            ACTIONS
                .iter()
                .enumerate()
                .map(|(k, a)| {
                    let local = if l == Lang::En { String::new() } else { format!(" {} {}", a.title.t(), a.desc.t()) };
                    format!("{} {} {} {}{local}", a.title.en(), a.id, a.desc.en(), self.text(k)).to_lowercase()
                })
                .collect()
        });
        &all[i]
    }

    pub fn keys(&self, i: usize) -> &[Chord] {
        self.custom.get(i).and_then(Option::as_deref).unwrap_or(ACTIONS[i].keys)
    }

    pub fn owner(&self, c: Chord) -> Option<usize> {
        self.owners.get(&c).copied()
    }

    pub fn runs(&self, key: Key, mods: Mods, id: &str) -> bool {
        self.owner(Chord { key, mods }).is_some_and(|i| ACTIONS[i].id == id)
    }

    pub fn is_custom(&self, i: usize) -> bool {
        self.custom.get(i).is_some_and(Option::is_some)
    }

    pub fn text(&self, i: usize) -> String {
        self.keys(i).iter().map(chord_text).collect::<Vec<_>>().join(", ")
    }

    pub fn shown(&self, i: usize) -> Vec<String> {
        ACTIONS[i].shown_keys(self.keys(i))
    }

    pub fn text_of(&self, id: &str) -> String {
        index(id).map(|i| self.text(i)).unwrap_or_default()
    }

    pub fn all_text(&self, id: &str) -> String {
        index(id).map(|i| self.shown(i).into_iter().filter(|k| !k.contains("Keypad")).collect::<Vec<_>>().join("  ")).unwrap_or_default()
    }

    pub fn first_text(&self, id: &str) -> Option<String> {
        index(id).and_then(|i| self.keys(i).first()).map(chord_text)
    }

    fn put(&mut self, i: usize, keys: Vec<Chord>) {
        self.custom[i] = (keys != ACTIONS[i].keys).then_some(keys);
    }

    fn take(&mut self, c: Chord, keep: usize) -> Option<usize> {
        let from = self.owner(c).filter(|&j| j != keep)?;
        let rest: Vec<Chord> = self.keys(from).iter().copied().filter(|&k| k != c).collect();
        self.put(from, rest);
        Some(from)
    }

    pub fn assign(&mut self, i: usize, old: Option<Chord>, c: Chord) -> Option<usize> {
        let from = self.take(c, i);
        let mut keys = self.keys(i).to_vec();
        match old.and_then(|o| keys.iter().position(|&k| k == o)) {
            Some(p) => keys[p] = c,
            None => keys.push(c),
        }
        let mut seen = Vec::with_capacity(keys.len());
        keys.retain(|k| {
            let fresh = !seen.contains(k);
            seen.push(*k);
            fresh
        });
        self.put(i, keys);
        self.reindex();
        from
    }

    pub fn remove(&mut self, i: usize, c: Chord) {
        let rest: Vec<Chord> = self.keys(i).iter().copied().filter(|&k| k != c).collect();
        self.put(i, rest);
        self.reindex();
    }

    pub fn restore(&mut self, i: usize) -> Vec<usize> {
        let taken = ACTIONS[i].keys.iter().filter_map(|&c| self.take(c, i)).collect();
        self.custom[i] = None;
        self.reindex();
        taken
    }

    pub fn to_json(&self) -> Json {
        let mut m = self.unknown.clone();
        for (i, keys) in self.custom.iter().enumerate() {
            if let Some(keys) = keys {
                m.insert(ACTIONS[i].id.to_owned(), Json::Array(keys.iter().map(|c| Json::String(chord_text(c))).collect()));
            }
        }
        Json::Object(m)
    }
}

pub fn defaults() -> Arc<Keymap> {
    static DEFAULTS: OnceLock<Arc<Keymap>> = OnceLock::new();
    DEFAULTS.get_or_init(|| Arc::new(Keymap::new(vec![None; ACTIONS.len()], Map::new()))).clone()
}

pub fn parse_chord(s: &str) -> Option<Chord> {
    let mut rest = s.trim();
    let mut mods = Mods::default();
    loop {
        let lower = rest.to_ascii_lowercase();
        let (flag, len) = match () {
            _ if lower.starts_with("ctrl+") => (&mut mods.ctrl, 5),
            _ if lower.starts_with("shift+") => (&mut mods.shift, 6),
            _ if lower.starts_with("alt+") => (&mut mods.alt, 4),
            _ => break,
        };
        if std::mem::replace(flag, true) {
            return None;
        }
        rest = &rest[len..];
    }
    let lower = rest.to_ascii_lowercase();
    let key = match lower.as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "pgup" | "pageup" => Key::PageUp,
        "pgdn" | "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "del" | "delete" => Key::Delete,
        "ins" | "insert" => Key::Insert,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "menu" | "apps" => Key::Apps,
        "num+" => Key::NumPlus,
        "browserback" => Key::BrowserBack,
        "browserforward" => Key::BrowserForward,
        f if f.len() > 1 && f.starts_with('f') => match f[1..].parse() {
            Ok(n @ 1..=24) => Key::F(n),
            _ => return None,
        },
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some('+'), None) => Key::Char('='),
                (Some(c), None) if c.is_ascii_graphic() => Key::Char(c.to_ascii_uppercase()),
                _ => return None,
            }
        }
    };
    Some(Chord { key, mods })
}

pub fn refusal(c: Chord) -> Option<String> {
    let m = c.mods;
    if c.key == Key::Other {
        return Some(S::ErrorsKeymapUnknownKey.en().into());
    }
    if ACTIONS.iter().any(|a| a.keys.contains(&c)) {
        return None;
    }
    if matches!(c.key, Key::Char(_) | Key::Space) && !m.ctrl && !m.alt {
        return Some(S::ErrorsKeymapNeedsModifier.en().into());
    }
    match c.key {
        Key::Up | Key::Down | Key::Left | Key::Right | Key::PageUp | Key::PageDown | Key::Home | Key::End if !m.alt => Some(S::ErrorsKeymapMovesFocus.en().into()),
        _ => None,
    }
}

fn clash(custom: &[Option<Vec<Chord>>]) -> Option<(Chord, usize, usize)> {
    let mut seen = FxHashMap::default();
    for (i, own) in custom.iter().enumerate().filter(|&(i, _)| ACTIONS[i].rebindable()) {
        for &c in own.as_deref().unwrap_or(ACTIONS[i].keys) {
            if let Some(j) = seen.insert(c, i) {
                return Some((c, j, i));
            }
        }
    }
    None
}

pub fn parse(j: &Json) -> Option<(Arc<Keymap>, Vec<String>)> {
    let map = j.as_object()?;
    if map.is_empty() {
        return Some((defaults(), Vec::new()));
    }
    let mut custom: Vec<Option<Vec<Chord>>> = vec![None; ACTIONS.len()];
    let (mut unknown, mut problems) = (Map::new(), Vec::new());
    for (id, v) in map {
        let Some(i) = index(id).filter(|&i| ACTIONS[i].rebindable()) else {
            unknown.insert(id.clone(), v.clone());
            continue;
        };
        let Some(list) = v.as_array() else {
            problems.push(tr_en!(S::ErrorsKeymapNotList, id = id));
            continue;
        };
        let mut keys = Vec::new();
        let mut bad = None;
        for item in list {
            match item.as_str().and_then(parse_chord) {
                None => bad = Some(tr_en!(S::ErrorsKeymapNotKey, item = item, id = id)),
                Some(c) => match refusal(c) {
                    Some(why) => bad = Some(tr_en!(S::ErrorsKeymapRefused, key = chord_text(&c), id = id, why = why)),
                    None if !keys.contains(&c) => keys.push(c),
                    None => {}
                },
            }
            if bad.is_some() {
                break;
            }
        }
        match bad {
            Some(p) => problems.push(p),
            None => custom[i] = (keys != ACTIONS[i].keys).then_some(keys),
        }
    }
    while let Some((c, a, b)) = clash(&custom) {
        if custom[a].is_none() && custom[b].is_none() {
            break;
        }
        problems.push(tr_en!(S::ErrorsKeymapClash, key = chord_text(&c), first = ACTIONS[a].id, second = ACTIONS[b].id));
        custom[a] = None;
        custom[b] = None;
    }
    match custom.iter().all(Option::is_none) && unknown.is_empty() {
        true => Some((defaults(), problems)),
        false => Some((Arc::new(Keymap::new(custom, unknown)), problems)),
    }
}

impl App {
    pub fn keymap(&self) -> &Arc<Keymap> {
        &self.keymap
    }

    pub(super) fn set_keymap(&mut self, k: Arc<Keymap>) {
        self.keymap = k;
        self.dirty = true;
    }

    pub(super) fn edit_keys<R>(&mut self, f: impl FnOnce(&mut Keymap) -> R) -> R {
        let r = f(Arc::make_mut(&mut self.keymap));
        self.layout();
        self.dirty = true;
        r
    }

    fn rest_text(&self, j: usize, without: Option<Chord>) -> String {
        let rest: Vec<String> = self.keymap.keys(j).iter().filter(|&&k| Some(k) != without).map(chord_text).collect();
        let command = ACTIONS[j].title;
        match rest.is_empty() {
            true => tr!(S::KeymapStatusLeftWithout, command = command),
            false => tr!(S::KeymapStatusKeeps, command = command, keys = rest.join(", ")),
        }
    }

    fn assign_chord(&mut self, i: usize, old: Option<Chord>, c: Chord) {
        let rest = self.keymap.owner(c).filter(|&j| j != i).map(|j| format!("; {}", self.rest_text(j, Some(c))));
        self.edit_keys(|k| k.assign(i, old, c));
        self.status(tr!(S::KeymapStatusAssigned, key = chord_text(&c), command = ACTIONS[i].title) + &rest.unwrap_or_default());
    }

    pub(super) fn remove_chord(&mut self, i: usize, c: Chord) {
        self.edit_keys(|k| k.remove(i, c));
        let title = ACTIONS[i].title;
        match self.keymap.keys(i).is_empty() {
            true => self.status(tr!(S::KeymapStatusNoShortcut, command = title)),
            false => self.status(tr!(S::KeymapStatusRemoved, key = chord_text(&c), command = title)),
        }
    }

    pub(super) fn restore_keys(&mut self, i: usize) {
        let taken = self.edit_keys(|k| k.restore(i));
        let mut note = tr!(S::KeymapStatusRestored, command = ACTIONS[i].title);
        for j in taken {
            note.push_str(&format!("; {}", self.rest_text(j, None)));
        }
        self.status(note);
    }

    pub(super) fn restore_all_keys(&mut self) {
        self.keymap = defaults();
        self.layout();
        self.dirty = true;
        self.status(S::KeymapStatusRestoredAll.t());
    }

    pub(super) fn open_capture(&mut self, action: usize, old: Option<Chord>) {
        self.show_capture(action, old, None);
    }

    fn show_capture(&mut self, action: usize, old: Option<Chord>, chord: Option<Chord>) {
        let title = ACTIONS[action].title;
        let mut rows: Vec<(String, String)> = Vec::new();
        match chord {
            None => rows.push((tr!(S::KeymapCapturePress, command = title), S::KeymapCaptureEscCancels.t().into())),
            Some(c) => {
                let name = chord_text(&c);
                rows.push(match (refusal(c), self.keymap.owner(c)) {
                    (Some(why), _) => (tr!(S::KeymapCaptureCannot, key = name, why = i18n::localize(&why)), S::KeymapCaptureOtherKeys.t().into()),
                    (None, Some(j)) if j == action => (tr!(S::KeymapCaptureAlready, key = name, command = title), String::new()),
                    (None, Some(j)) => (tr!(S::KeymapCaptureReassign, key = name, from = ACTIONS[j].title, command = title), self.rest_text(j, Some(c))),
                    (None, None) => (tr!(S::KeymapCaptureAssign, key = name, command = title), "Enter".into()),
                });
                rows.push((S::SettingsPageDialogCancel.t().into(), "Esc".into()));
            }
        }
        if let Some(o) = old {
            rows.push((tr!(S::KeymapCaptureRemove, key = chord_text(&o)), String::new()));
        }
        let text = chord.map(|c| chord_text(&c)).unwrap_or_default();
        self.nav.palette = Some(Palette::new(Pick::Setting(SettingPick::Capture { action, old, chord }), rows, &text));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn capture_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let Some(Pick::Setting(SettingPick::Capture { action, old, .. })) = self.nav.palette.as_ref().map(|p| &p.pick) else { return false };
        let (action, old) = (*action, *old);
        let plain = m == Mods::default();
        match key {
            Key::Escape if plain => self.close_palette(),
            Key::Enter if plain => self.run_palette_item(0, now),
            Key::Other => {}
            _ => self.show_capture(action, old, Some(Chord { key, mods: m })),
        }
        self.layout();
        true
    }

    pub(super) fn capture_choose(&mut self, action: usize, old: Option<Chord>, chord: Option<Chord>, row: usize) {
        let remove_row = if chord.is_some() { 2 } else { 1 };
        match (chord, old) {
            (_, Some(o)) if row == remove_row => self.remove_chord(action, o),
            (Some(c), _) if row == 0 && refusal(c).is_none() => {
                if self.keymap.owner(c) != Some(action) {
                    self.assign_chord(action, old, c);
                }
            }
            _ if row == 0 => self.show_capture(action, old, chord),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/keymap.rs"]
mod tests;
