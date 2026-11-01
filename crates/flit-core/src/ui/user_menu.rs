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
}

impl Default for UserEntry {
    fn default() -> Self {
        Self { name: String::new(), icon: String::new(), command: String::new(), files: true, folders: true, background: false, ext: String::new(), submenu: None }
    }
}

pub fn glyph(icon: &str) -> Option<&'static str> {
    GLYPHS.iter().find(|g| g.0 == icon).map(|g| g.1)
}

fn ext_of(name: &[u8]) -> String {
    let dot = name.iter().rposition(|&b| b == b'.').filter(|&i| i > 0);
    dot.map(|i| String::from_utf8_lossy(&name[i + 1..]).to_lowercase()).unwrap_or_default()
}

impl UserEntry {
    fn exts(&self) -> Vec<String> {
        self.ext.split([' ', ',', ';']).map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect()
    }

    pub fn wants(&self, items: Option<&[(bool, &[u8])]>) -> bool {
        let Some(items) = items else { return self.background };
        let exts = self.exts();
        !items.is_empty() && items.iter().all(|&(dir, name)| if dir { self.folders } else { self.files && (exts.is_empty() || exts.contains(&ext_of(name))) })
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
            None => m.insert("command".into(), Json::String(self.command.clone())),
        };
        let show = [("files", self.files), ("folders", self.folders), ("background", self.background)];
        m.insert("show".into(), Json::Array(show.iter().filter(|s| s.1).map(|s| Json::String(s.0.into())).collect()));
        if !self.ext.is_empty() {
            m.insert("extensions".into(), Json::String(self.ext.clone()));
        }
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
        let e = UserEntry {
            name: text("name")?,
            icon: text("icon")?,
            command: text("command")?,
            files: shown.contains(&"files"),
            folders: shown.contains(&"folders"),
            background: shown.contains(&"background"),
            ext: text("extensions")?,
            submenu,
        };
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
            let image = f.text(IMAGE).trim().trim_matches('"').to_owned();
            e.icon = match (image.is_empty(), f.choice(ICON)) {
                (false, _) => image,
                (true, 0) => String::new(),
                (true, k) => GLYPHS.get(k - 1).map_or_else(String::new, |g| g.0.to_owned()),
            };
            parent = f.choice(PLACE).checked_sub(1).and_then(|k| self.submenus().get(k).copied());
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

fn push(m: &mut Menu, e: &UserEntry, places: &Places, dir: &FsPath) {
    let Some((program, args)) = cmdline::expand(&e.command, cmdline::POSIX, places) else { return };
    let launch = Launch::Run { program, args: cmdline::join(&args, cmdline::POSIX), dir: Some(dir.clone()) };
    m.user(&e.name, glyph(&e.icon).unwrap_or(""), e.image(), launch);
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
