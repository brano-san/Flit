use super::archive::Kind;
use super::form::{Control, Form};
use super::menu::{Menu, MenuImage, MenuItem};
use super::widgets::baseline;
use super::{App, IconKind, IconRequest};
use crate::cmdline::{self, Places};
use crate::i18n::S;
use crate::path::FsPath;
use crate::provider::Launch;
use crate::render::Rect;
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use serde_json::{Map, Value as Json};

pub const MAX_ENTRIES: usize = 64;
const MAX_TEXT: usize = 4096;
const ICON_FILES: [&str; 7] = ["ico", "exe", "dll", "lnk", "icl", "cpl", "msc"];
const NAME: usize = 0;
const ICON: usize = 1;
const IMAGE: usize = 2;
const COMMAND: usize = 3;
const PLACE: usize = 4;
const SHOW: usize = 5;
const EXT: usize = 6;
const DESCRIPTION: usize = 7;
const SHORTCUT: usize = 8;
const NOTIFY: usize = 9;
const PATTERNS: usize = 10;
const SELECTION: usize = 11;
const CATEGORIES: usize = 12;
const SUB_SHOW: usize = 1;
const SUB_EXT: usize = 2;

pub const GLYPHS: [(&str, &str); 32] = [
    ("terminal", "\u{E756}"),
    ("document", "\u{E7C3}"),
    ("text", "\u{E8A5}"),
    ("folder", "\u{E8B7}"),
    ("new_folder", "\u{E8F4}"),
    ("image", "\u{E8B9}"),
    ("music", "\u{E8D6}"),
    ("video", "\u{E714}"),
    ("play", "\u{E768}"),
    ("copy", "\u{E8C8}"),
    ("rename", "\u{E8AC}"),
    ("delete", "\u{E74D}"),
    ("link", "\u{E71B}"),
    ("share", "\u{E72D}"),
    ("search", "\u{E721}"),
    ("settings", "\u{E713}"),
    ("tag", "\u{E8EC}"),
    ("box", "\u{E7B8}"),
    ("drive", "\u{EDA2}"),
    ("info", "\u{E946}"),
    ("home", "\u{E80F}"),
    ("desktop", "\u{E7F4}"),
    ("download", "\u{E896}"),
    ("refresh", "\u{E72C}"),
    ("eye", "\u{E890}"),
    ("filter", "\u{E71C}"),
    ("grid", "\u{E8A9}"),
    ("list", "\u{E8FD}"),
    ("library", "\u{E8F1}"),
    ("history", "\u{E81C}"),
    ("pin", "\u{E718}"),
    ("arrow", "\u{E72A}"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct UserEntry {
    pub name: String,
    pub icon: String,
    pub command: String,
    pub files: bool,
    pub folders: bool,
    pub background: bool,
    pub ext: String,
    pub submenu: Option<Vec<UserEntry>>,
    pub description: String,
    pub shortcut: String,
    pub startup_notify: bool,
    pub patterns: String,
    pub selection: usize,
    pub categories: Vec<String>,
}

impl Default for UserEntry {
    fn default() -> Self {
        Self { name: String::new(), icon: String::new(), command: String::new(), files: true, folders: true, background: false, ext: String::new(), submenu: None, description: String::new(), shortcut: String::new(), startup_notify: false, patterns: String::new(), selection: 0, categories: Vec::new() }
    }
}

pub fn action_chord(text: &str) -> Option<super::actions::Chord> {
    let c = super::keymap::parse_chord(text)?;
    (c.mods.ctrl || c.mods.alt || matches!(c.key, crate::input::Key::F(_))).then_some(c)
}

pub fn glyph(icon: &str) -> Option<&'static str> {
    GLYPHS.iter().find(|g| g.0 == icon).map(|g| g.1)
}

fn ext_of(name: &[u8]) -> String {
    let dot = name.iter().rposition(|&b| b == b'.').filter(|&i| i > 0);
    dot.map(|i| String::from_utf8_lossy(&name[i + 1..]).to_lowercase()).unwrap_or_default()
}

const CATEGORY_IDS: [&str; 6] = ["folder", "audio", "video", "image", "text", "other"];

fn category(dir: bool, ext: &str) -> &'static str {
    if dir { "folder" }
    else if ["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "wma"].contains(&ext) { "audio" }
    else if ["mp4", "mkv", "webm", "avi", "mov", "mpeg", "mpg", "wmv"].contains(&ext) { "video" }
    else if crate::preview::IMAGES.contains(&ext) { "image" }
    else if ["txt", "md", "markdown", "csv", "json", "xml", "html", "css", "js", "ts", "rs", "py", "toml", "yaml", "yml", "log", "ini", "conf"].contains(&ext) { "text" }
    else { "other" }
}

fn pattern_matches(pattern: &str, name: &[u8]) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = String::from_utf8_lossy(name).to_lowercase().chars().collect();
    let (mut pi, mut ni, mut star) = (0, 0, None);
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => { star = Some((pi, ni)); pi += 1; }
            Some('?') => { pi += 1; ni += 1; }
            Some(c) if *c == n[ni] => { pi += 1; ni += 1; }
            _ => match star {
                Some((sp, sn)) => { let next = sn + 1; star = Some((sp, next)); pi = sp + 1; ni = next; }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

impl UserEntry {
    fn exts(&self) -> Vec<String> {
        self.ext.split([' ', ',', ';']).map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect()
    }

    pub fn wants(&self, items: Option<&[(bool, &[u8])]>) -> bool {
        let Some(items) = items else { return self.background };
        if (self.selection == 1 && items.len() != 1) || (self.selection == 2 && items.len() < 2) { return false; }
        let exts = self.exts();
        let patterns: Vec<_> = self.patterns.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
        !items.is_empty() && items.iter().all(|&(dir, name)| {
            let ext = ext_of(name);
            let category = category(dir, &ext);
            (if dir { self.folders } else { self.files && (exts.is_empty() || exts.contains(&ext)) })
                && (self.categories.is_empty() || self.categories.iter().any(|c| c == category))
                && (patterns.is_empty() || patterns.iter().any(|p| pattern_matches(p, name)))
        })
    }

    fn image(&self) -> Option<MenuImage> {
        if self.icon.is_empty() || glyph(&self.icon).is_some() {
            return None;
        }
        FsPath::parse(&self.icon).map(MenuImage::File)
    }

    pub fn to_json(&self) -> Json {
        let mut m = Map::new();
        m.insert("name".into(), Json::String(self.name.clone()));
        if !self.icon.is_empty() {
            m.insert("icon".into(), Json::String(self.icon.clone()));
        }
        match &self.submenu {
            Some(items) => m.insert("items".into(), Json::Array(items.iter().map(UserEntry::to_json).collect())),
            None => {
                m.insert("percent".into(), Json::Bool(true));
                m.insert("command".into(), Json::String(self.command.clone()))
            }
        };
        let show = [("files", self.files), ("folders", self.folders), ("background", self.background)];
        m.insert("show".into(), Json::Array(show.iter().filter(|s| s.1).map(|s| Json::String(s.0.into())).collect()));
        if !self.ext.is_empty() {
            m.insert("extensions".into(), Json::String(self.ext.clone()));
        }
        for (key, value) in [("description", &self.description), ("shortcut", &self.shortcut), ("patterns", &self.patterns)] {
            if !value.is_empty() { m.insert(key.into(), Json::String(value.clone())); }
        }
        if self.startup_notify { m.insert("startup_notify".into(), Json::Bool(true)); }
        if self.selection != 0 { m.insert("selection".into(), Json::from(self.selection)); }
        if !self.categories.is_empty() { m.insert("categories".into(), Json::Array(self.categories.iter().cloned().map(Json::String).collect())); }
        Json::Object(m)
    }

    fn from_json(j: &Json, nested: bool) -> Option<Self> {
        let o = j.as_object()?;
        let text = |k: &str| match o.get(k) {
            None => Some(String::new()),
            Some(v) => v.as_str().filter(|t| t.len() <= MAX_TEXT).map(str::to_owned),
        };
        let shown: Vec<&str> = match o.get("show") {
            None => vec!["files", "folders"],
            Some(v) => v.as_array()?.iter().map(Json::as_str).collect::<Option<_>>()?,
        };
        let submenu = match o.get("items") {
            Some(_) if nested => return None,
            Some(v) => Some(v.as_array().filter(|a| a.len() <= MAX_ENTRIES)?.iter().map(|c| UserEntry::from_json(c, true)).collect::<Option<Vec<_>>>()?),
            None => None,
        };
        let command = text("command")?;
        let percent = o.get("percent").map_or(Some(false), Json::as_bool)?;
        let mut e = UserEntry {
            name: text("name")?,
            icon: text("icon")?,
            command: if percent { command } else { command.replace('%', "%%") },
            files: shown.contains(&"files"),
            folders: shown.contains(&"folders"),
            background: shown.contains(&"background"),
            ext: text("extensions")?,
            submenu,
            description: text("description")?,
            shortcut: text("shortcut")?,
            patterns: text("patterns")?,
            startup_notify: o.get("startup_notify").and_then(Json::as_bool).unwrap_or(false),
            selection: o.get("selection").and_then(Json::as_u64).filter(|&s| s <= 2).map_or(0, |s| s as usize),
            categories: o.get("categories").and_then(Json::as_array).map(|a| a.iter().filter_map(|c| c.as_str().filter(|c| !c.is_empty() && c.len() <= MAX_TEXT)).map(str::to_owned).collect()).unwrap_or_default(),
        };
        if action_chord(&e.shortcut).is_none() { e.shortcut.clear(); }
        (!e.name.trim().is_empty() && (e.submenu.is_some() || !e.command.trim().is_empty())).then_some(e)
    }
}

pub fn to_json(list: &[UserEntry]) -> Json {
    Json::Array(list.iter().map(UserEntry::to_json).collect())
}

pub fn from_json(j: &Json) -> Option<Vec<UserEntry>> {
    j.as_array().filter(|a| a.len() <= MAX_ENTRIES)?.iter().map(|e| UserEntry::from_json(e, false)).collect()
}

pub fn image_request(image: &MenuImage, px: u32) -> Option<IconRequest> {
    let MenuImage::File(path) = image else { return None };
    let kind = if ICON_FILES.contains(&ext_of(path.file_name()).as_str()) { IconKind::File } else { IconKind::Thumbnail };
    Some(IconRequest { key: image.key(px), kind, path: path.clone(), size: px, gen: 0, row: 0 })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EntryAt {
    pub top: usize,
    pub child: Option<usize>,
}

impl App {
    fn user_actions(&self) -> impl Iterator<Item = (EntryAt, Option<&UserEntry>, &UserEntry)> {
        self.opts.user_menu.iter().enumerate().flat_map(|(top, e)| match &e.submenu {
            Some(items) => items.iter().enumerate().map(|(child, c)| (EntryAt { top, child: Some(child) }, Some(e), c)).collect::<Vec<_>>(),
            None => vec![(EntryAt { top, child: None }, None, e)],
        })
    }

    pub(super) fn user_shortcut(&mut self, key: crate::input::Key, mods: crate::input::Mods) -> bool {
        let pressed = super::actions::Chord { key, mods };
        let matching: Vec<_> = self.user_actions().filter(|(_, _, e)| !e.shortcut.is_empty() && action_chord(&e.shortcut) == Some(pressed)).map(|(_, parent, e)| (parent, e)).collect();
        if matching.is_empty() || self.archive_view().is_some() { return false; }
        let t = self.tab();
        let (Some(v), Some(dir)) = (&t.view, t.view_loc.to_windows()) else { return false };
        let ids: Vec<_> = v.selected().collect();
        let listing = v.listing();
        let kinds: Vec<_> = ids.iter().map(|&i| (listing.entries[i as usize].is_dir(), listing.name(i))).collect();
        let target = (!ids.is_empty()).then_some(kinds.as_slice());
        let Some(entry) = matching.into_iter().find(|(parent, e)| parent.is_none_or(|p| p.wants(target)) && e.wants(target)).map(|(_, e)| e) else { return false };
        let Ok(paths) = self.item_paths(&ids) else { return false };
        let shown: Vec<_> = paths.iter().map(|p| p.display().into_owned()).collect();
        let here = dir.display().into_owned();
        let Some(launch) = entry_launch(entry, &Places { items: &shown, dir: &here }, &dir) else { return false };
        self.launch(launch);
        true
    }

    pub(super) fn user_entries(&self, m: &mut Menu, on_items: bool) {
        let list = &self.opts.user_menu;
        let t = self.tab();
        let (Some(v), Some(dir)) = (&t.view, t.view_loc.to_windows()) else { return };
        if list.is_empty() || self.archive_view().is_some() {
            return;
        }
        let ids: Vec<u32> = if on_items { v.selected().collect() } else { Vec::new() };
        let Ok(paths) = self.item_paths(&ids) else { return };
        let l = v.listing();
        let kinds: Vec<(bool, &[u8])> = ids.iter().map(|&i| (l.entries[i as usize].is_dir(), l.name(i))).collect();
        let shown: Vec<String> = paths.iter().map(|p| p.display().into_owned()).collect();
        let here = dir.display().into_owned();
        let places = Places { items: &shown, dir: &here };
        let target = on_items.then_some(kinds.as_slice());
        for e in list.iter().filter(|e| e.wants(target)) {
            match &e.submenu {
                Some(items) => m.submenu(&e.name, |m| items.iter().filter(|c| c.wants(target)).for_each(|c| push(m, c, &places, &dir))),
                None => push(m, e, &places, &dir),
            }
        }
    }

    pub(super) fn request_menu_images(&mut self, images: &[MenuImage], px: u32, has: impl Fn(u64) -> bool) {
        for r in images.iter().filter_map(|i| image_request(i, px)) {
            if !has(r.key) && !self.icons.contains_key(&r.key) {
                self.icons.insert(r.key, super::IconState::Pending);
                self.effects.push(super::Effect::Icon(r));
            }
        }
    }

    pub(super) fn user_lines(&self) -> Vec<EntryAt> {
        let mut out = Vec::new();
        for (top, e) in self.opts.user_menu.iter().enumerate() {
            out.push(EntryAt { top, child: None });
            out.extend((0..e.submenu.as_ref().map_or(0, Vec::len)).map(|c| EntryAt { top, child: Some(c) }));
        }
        out
    }

    fn user_entry(&self, at: EntryAt) -> Option<&UserEntry> {
        let top = self.opts.user_menu.get(at.top)?;
        match at.child {
            Some(c) => top.submenu.as_ref()?.get(c),
            None => Some(top),
        }
    }

    pub(super) fn user_entry_name(&self, at: EntryAt) -> &str {
        self.user_entry(at).map_or("", |e| e.name.as_str())
    }

    pub(super) fn user_line_text(&self, at: EntryAt) -> String {
        self.user_entry(at).map_or_else(String::new, |e| format!("entry {}{}", if at.child.is_some() { "  " } else { "" }, e.name))
    }

    pub(super) fn draw_user_line<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, r: Rect, at: EntryAt, fonts: (FontStyle, FontStyle, FontStyle)) {
        let Some(e) = self.user_entry(at) else { return };
        let (font, small, icons) = fonts;
        let s = |v: f32| (v * self.scale).round();
        let x = r.x + s(16.0) + if at.child.is_some() { s(22.0) } else { 0.0 };
        let icon = Rect::new(x, r.y + (r.h - s(20.0)) / 2.0, s(20.0), s(20.0));
        let drawn = e.image().is_some_and(|i| super::context::draw_image(g, &i, icon, self.scale, 0xFF));
        let glyph = match &e.submenu {
            Some(_) => Some("\u{E8B7}"),
            None => glyph(&e.icon),
        };
        if let (false, Some(gl)) = (drawn, glyph) {
            super::centered(g, icon, gl, icons, t.text_dim);
        }
        let tx = icon.right() + s(10.0);
        let (y, small_y) = (baseline(g, r, font), baseline(g, r, small));
        let w = g.text(tx, y, &e.name, font, t.text, s(240.0));
        let detail = match &e.submenu {
            Some(items) => S::UserMenuSubmenu.t().to_owned() + &format!(" \u{00B7} {}", items.len()),
            None if !e.description.is_empty() => e.description.clone(),
            None => e.command.clone(),
        };
        let room = r.right() - s(16.0) - 3.0 * (s(92.0) + s(8.0)) - (tx + w + s(14.0));
        g.text(tx + w + s(14.0), small_y, &detail, small, t.text_dim, room.max(0.0));
    }

    pub(super) fn request_user_line_images<B: TextBackend>(&mut self, g: &Gfx<B>) {
        let px = super::context::image_px(self.scale);
        let images: Vec<MenuImage> = self.opts.user_menu.iter().flat_map(|e| std::iter::once(e).chain(e.submenu.iter().flatten())).filter_map(UserEntry::image).collect();
        self.request_menu_images(&images, px, |k| g.has_image(k));
    }

    pub(super) fn new_user_entry(&mut self, submenu: bool) {
        if self.opts.user_menu.len() >= MAX_ENTRIES {
            return self.status(S::UserMenuTooMany.t());
        }
        self.open_user_form(None, submenu);
    }

    pub(super) fn edit_user_entry(&mut self, at: EntryAt) {
        let submenu = self.user_entry(at).is_some_and(|e| e.submenu.is_some());
        self.open_user_form(Some(at), submenu);
    }

    pub(super) fn delete_user_entry(&mut self, at: EntryAt) {
        let list = &mut self.opts.user_menu;
        match at.child {
            Some(c) => {
                if let Some(items) = list.get_mut(at.top).and_then(|e| e.submenu.as_mut()).filter(|i| c < i.len()) {
                    items.remove(c);
                }
            }
            None if at.top < list.len() => {
                list.remove(at.top);
            }
            None => {}
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn raise_user_entry(&mut self, at: EntryAt) {
        let list = &mut self.opts.user_menu;
        match at.child {
            Some(c) if c > 0 => {
                if let Some(items) = list.get_mut(at.top).and_then(|e| e.submenu.as_mut()).filter(|i| c < i.len()) {
                    items.swap(c - 1, c);
                }
            }
            None if at.top > 0 && at.top < list.len() => list.swap(at.top - 1, at.top),
            _ => {}
        }
        self.layout();
        self.dirty = true;
    }

    fn submenus(&self) -> Vec<usize> {
        self.opts.user_menu.iter().enumerate().filter(|(_, e)| e.submenu.is_some()).map(|(k, _)| k).collect()
    }

    fn open_user_form(&mut self, at: Option<EntryAt>, submenu: bool) {
        let e = at.and_then(|a| self.user_entry(a)).cloned().unwrap_or_default();
        let shows = vec![(S::UserMenuFiles.t().to_owned(), e.files), (S::UserMenuFolders.t().to_owned(), e.folders), (S::UserMenuBackground.t().to_owned(), e.background)];
        let mut controls = vec![Control::text(S::UserMenuName.t(), &e.name, S::UserMenuNameHint.t())];
        if !submenu {
            let mut icons = vec![S::UserMenuNoIcon.t().to_owned()];
            icons.extend(GLYPHS.iter().map(|g| g.0.replace('_', " ")));
            let sel = GLYPHS.iter().position(|g| g.0 == e.icon).map_or(0, |k| k + 1);
            let image = if glyph(&e.icon).is_some() { "" } else { e.icon.as_str() };
            let mut places = vec![S::UserMenuTopLevel.t().to_owned()];
            let subs = self.submenus();
            places.extend(subs.iter().map(|&k| self.opts.user_menu[k].name.clone()));
            let place = at.and_then(|a| a.child.and_then(|_| subs.iter().position(|&k| k == a.top))).map_or(0, |k| k + 1);
            controls.push(Control::choice(S::UserMenuIcon.t(), icons, sel));
            controls.push(Control::text(S::UserMenuImage.t(), image, S::UserMenuImageHint.t()));
            controls.push(Control::text(S::UserMenuCommand.t(), &e.command, S::UserMenuCommandHint.t()));
            controls.push(Control::choice(S::UserMenuPlace.t(), places, place));
        }
        controls.push(Control::flags(S::UserMenuShow.t(), shows));
        controls.push(Control::text(S::UserMenuExtensions.t(), &e.ext, S::UserMenuExtensionsHint.t()));
        if !submenu {
            controls.push(Control::text(S::UserMenuDescription.t(), &e.description, ""));
            controls.push(Control::text(S::UserMenuShortcut.t(), &e.shortcut, "Ctrl+Shift+H"));
            controls.push(Control::check(S::UserMenuStartupNotify.t(), e.startup_notify));
            controls.push(Control::text(S::UserMenuPatterns.t(), &e.patterns, "*.txt; *.md"));
            controls.push(Control::choice(S::UserMenuSelection.t(), vec![S::UserMenuAnySelection.t().into(), S::UserMenuSingleSelection.t().into(), S::UserMenuMultipleSelection.t().into()], e.selection));
            let labels = [S::UserMenuFolders, S::UserMenuAudio, S::UserMenuVideo, S::UserMenuImages, S::UserMenuText, S::UserMenuOther];
            controls.push(Control::flags(S::UserMenuCategories.t(), labels.iter().zip(CATEGORY_IDS).map(|(label, id)| (label.t().to_owned(), e.categories.is_empty() || e.categories.iter().any(|c| c == id))).collect()));
        }
        let title = match (submenu, at.is_some()) {
            (true, _) => S::UserMenuSubmenuTitle.t(),
            (false, false) => S::UserMenuNewTitle.t(),
            (false, true) => S::UserMenuEditTitle.t(),
        };
        let mut f = Form::new(Kind::UserMenu { at, submenu }, title, controls, vec![S::UserMenuSave.t(), S::SettingsPageDialogCancel.t()]);
        if !submenu {
            f.say(S::UserMenuPlaceholders.t(), false);
        }
        self.open_form(f);
    }

    pub(super) fn submit_user_entry(&mut self, mut f: Form) -> Option<Form> {
        let Kind::UserMenu { at, submenu } = f.kind else { return None };        let name = f.text(NAME).trim().to_owned();
        if name.is_empty() {
            f.say(S::UserMenuNoName.t(), true);
            return Some(f);
        }
        let old = at.and_then(|a| self.user_entry(a)).cloned();
        let (show, ext) = if submenu { (SUB_SHOW, SUB_EXT) } else { (SHOW, EXT) };
        let flags = f.flags(show);
        let mut e = UserEntry { name, ext: f.text(ext).trim().to_owned(), files: flags.first() == Some(&true), folders: flags.get(1) == Some(&true), background: flags.get(2) == Some(&true), ..UserEntry::default() };
        let mut parent = None;
        if submenu {
            e.submenu = Some(old.and_then(|o| o.submenu).unwrap_or_default());
        } else {
            e.command = f.text(COMMAND).trim().to_owned();
            if cmdline::expand(&e.command, cmdline::POSIX, &Places { items: &[], dir: "." }).is_none() {
                f.say(S::UserMenuNoCommand.t(), true);
                return Some(f);
            }
            if cmdline::expand(&e.command, cmdline::POSIX, &Places { items: &["a".into(), "b".into()], dir: "." }).is_none() {
                f.say(S::UserMenuBadCommand.t(), true);
                return Some(f);
            }
            let image = f.text(IMAGE).trim().trim_matches('"').to_owned();
            e.icon = match (image.is_empty(), f.choice(ICON)) {
                (false, _) => image,
                (true, 0) => String::new(),
                (true, k) => GLYPHS.get(k - 1).map_or_else(String::new, |g| g.0.to_owned()),
            };
            parent = f.choice(PLACE).checked_sub(1).and_then(|k| self.submenus().get(k).copied());
            e.description = f.text(DESCRIPTION).trim().to_owned();
            e.shortcut = f.text(SHORTCUT).trim().to_owned();
            if !e.shortcut.is_empty() {
                let Some(chord) = action_chord(&e.shortcut) else {
                    f.say(S::UserMenuBadShortcut.t(), true);
                    return Some(f);
                };
                let builtin = self.keymap.owner(chord).filter(|&i| self.supports(super::actions::ACTIONS[i].id)).map(|i| super::actions::ACTIONS[i].title.t().to_owned());
                let custom = || self.user_actions().find(|(other, _, o)| Some(*other) != at && action_chord(&o.shortcut) == Some(chord)).map(|(_, _, o)| o.name.clone());
                if let Some(command) = builtin.or_else(custom) {
                    f.say(crate::tr!(S::KeymapCaptureAlready, key = &e.shortcut, command = command), true);
                    return Some(f);
                }
            }
            e.startup_notify = f.checked(NOTIFY);
            e.patterns = f.text(PATTERNS).trim().to_owned();
            e.selection = f.choice(SELECTION);
            let categories = f.flags(CATEGORIES);
            if !categories.iter().any(|on| *on) { f.say(S::UserMenuNoCategories.t(), true); return Some(f); }
            if !categories.iter().all(|on| *on) { e.categories = CATEGORY_IDS.iter().zip(categories).filter(|(_, on)| *on).map(|(id, _)| (*id).to_owned()).collect(); }
        }
        let same_parent = at.is_some_and(|a| a.child.map(|_| a.top) == parent);
        let destination_len = parent.and_then(|p| self.opts.user_menu.get(p)).and_then(|p| p.submenu.as_ref()).map_or(self.opts.user_menu.len(), Vec::len);
        if !same_parent && destination_len >= MAX_ENTRIES {
            f.say(S::UserMenuTooMany.t(), true);
            return Some(f);
        }
        self.place_user_entry(at, parent, e);
        self.layout();
        self.dirty = true;
        None
    }

    fn place_user_entry(&mut self, at: Option<EntryAt>, parent: Option<usize>, e: UserEntry) {
        let list = &mut self.opts.user_menu;
        let same = at.is_some_and(|a| a.child.map(|_| a.top) == parent);
        match at {
            Some(EntryAt { top, child: Some(c) }) if same => {
                if let Some(slot) = list.get_mut(top).and_then(|p| p.submenu.as_mut()).and_then(|i| i.get_mut(c)) {
                    *slot = e;
                }
                return;
            }
            Some(EntryAt { top, child: None }) if same => {
                if let Some(slot) = list.get_mut(top) {
                    *slot = e;
                }
                return;
            }
            Some(EntryAt { top, child: Some(c) }) => {
                if let Some(items) = list.get_mut(top).and_then(|p| p.submenu.as_mut()).filter(|i| c < i.len()) {
                    items.remove(c);
                }
            }
            Some(EntryAt { top, child: None }) if top < list.len() => {
                list.remove(top);
            }
            _ => {}
        }
        let removed_before = matches!(at, Some(EntryAt { top, child: None }) if parent.is_some_and(|p| top < p));
        match parent.map(|p| if removed_before { p - 1 } else { p }).and_then(|p| list.get_mut(p)).and_then(|p| p.submenu.as_mut()) {
            Some(items) => items.push(e),
            None => list.push(e),
        }
    }
}

fn entry_launch(e: &UserEntry, places: &Places, dir: &FsPath) -> Option<Launch> {
    let (program, args) = cmdline::expand(&e.command, cmdline::POSIX, places)?;
    let mut argv = vec![program];
    argv.extend(args);
    let env = vec![("FLIT_STARTUP_NOTIFY".to_owned(), if e.startup_notify { "1" } else { "0" }.to_owned())];
    Some(Launch::Command { runs: vec![(argv, env)], dir: Some(dir.clone()) })
}

fn push(m: &mut Menu, e: &UserEntry, places: &Places, dir: &FsPath) {
    let Some(launch) = entry_launch(e, places, dir) else { return };
    m.user(&e.name, &e.shortcut, glyph(&e.icon).unwrap_or(""), e.image(), launch);
}

pub(super) fn file_images(items: &[MenuItem], out: &mut Vec<MenuImage>) {
    for i in items {
        match i {
            MenuItem::Command { image: Some(image @ MenuImage::File(_)), .. } => out.push(image.clone()),
            MenuItem::Submenu { items, .. } => file_images(items, out),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/user_menu.rs"]
mod tests;
