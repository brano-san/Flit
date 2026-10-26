use crate::dbus::{self, text, Api, Iter};
use flit_core::a11y::{Node, Role, Tree, ROOT_ID};
use flit_core::input::Event;
use flit_platform::WindowId;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard};

const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
const NULL_PATH: &str = "/org/a11y/atspi/null";
const NODE_PATH: &str = "/org/a11y/atspi/accessible/w";
const BUS_NAME: &str = "org.a11y.Bus";
const BUS_PATH: &str = "/org/a11y/bus";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const APPLICATION: &str = "org.a11y.atspi.Application";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const SELECTION: &str = "org.a11y.atspi.Selection";
const TEXT: &str = "org.a11y.atspi.Text";
const TABLE: &str = "org.a11y.atspi.Table";
const CACHE: &str = "org.a11y.atspi.Cache";
const CACHE_ITEM: &str = "((so)(so)(so)iiassusau)";
const OBJECT_EVENTS: &str = "org.a11y.atspi.Event.Object";
const FOCUS_EVENTS: &str = "org.a11y.atspi.Event.Focus";
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";
const UNKNOWN_PROPERTY: &str = "org.freedesktop.DBus.Error.UnknownProperty";
const STATUS_MATCH: &str = "type='signal',sender='org.a11y.Bus',path='/org/a11y/bus',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'";
const TOOLKIT: &str = "Flit";
const CALL_TIMEOUT_MS: c_int = 5000;
const TYPE_METHOD_CALL: c_int = 1;
const TYPE_SIGNAL: c_int = 4;
const DATA_REMAINS: c_int = 0;
const ROLE_APPLICATION: (u32, &str) = (75, "application");
const LAYER_WIDGET: u32 = 3;
const LAYER_WINDOW: u32 = 7;

const PROPS: &[(&str, &str)] = &[
    (ACCESSIBLE, "Name"),
    (ACCESSIBLE, "Description"),
    (ACCESSIBLE, "Parent"),
    (ACCESSIBLE, "ChildCount"),
    (ACCESSIBLE, "Locale"),
    (ACCESSIBLE, "AccessibleId"),
    (ACCESSIBLE, "HelpText"),
    (APPLICATION, "ToolkitName"),
    (APPLICATION, "Version"),
    (APPLICATION, "AtspiVersion"),
    (APPLICATION, "Id"),
    (ACTION, "NActions"),
    (SELECTION, "NSelectedChildren"),
    (TEXT, "CharacterCount"),
    (TEXT, "CaretOffset"),
    (TABLE, "NRows"),
    (TABLE, "NColumns"),
    (TABLE, "Caption"),
    (TABLE, "Summary"),
    (TABLE, "NSelectedRows"),
    (TABLE, "NSelectedColumns"),
];

mod state {
    pub const ACTIVE: u32 = 1;
    pub const CHECKED: u32 = 4;
    pub const EDITABLE: u32 = 7;
    pub const ENABLED: u32 = 8;
    pub const FOCUSABLE: u32 = 11;
    pub const FOCUSED: u32 = 12;
    pub const MODAL: u32 = 16;
    pub const MULTISELECTABLE: u32 = 18;
    pub const SELECTABLE: u32 = 22;
    pub const SELECTED: u32 = 23;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const SINGLE_LINE: u32 = 26;
    pub const VISIBLE: u32 = 30;
    pub const CHECKABLE: u32 = 41;
    pub const READ_ONLY: u32 = 43;
}

static LISTENING: AtomicBool = AtomicBool::new(false);
static WAKE: AtomicI32 = AtomicI32::new(-1);
static HUB: Mutex<Hub> = Mutex::new(Hub { windows: Vec::new(), events: Vec::new() });

struct Win {
    id: WindowId,
    origin: (i32, i32),
    tree: Tree,
}

struct Hub {
    windows: Vec<Win>,
    events: Vec<Ev>,
}

fn hub() -> MutexGuard<'static, Hub> {
    HUB.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ref {
    Null,
    App,
    Node(WindowId, u64),
    Other(String, String),
}

impl Ref {
    pub(crate) fn path(&self) -> String {
        match self {
            Ref::Null => NULL_PATH.into(),
            Ref::App => ROOT_PATH.into(),
            Ref::Node(w, id) => format!("{NODE_PATH}{w}n{id:x}"),
            Ref::Other(_, p) => p.clone(),
        }
    }

    pub(crate) fn parse(path: &str) -> Option<Ref> {
        if path == ROOT_PATH {
            return Some(Ref::App);
        }
        let (w, id) = path.strip_prefix(NODE_PATH)?.split_once('n')?;
        Some(Ref::Node(w.parse().ok()?, u64::from_str_radix(id, 16).ok()?))
    }

    fn bus<'a>(&'a self, me: &'a str) -> &'a str {
        match self {
            Ref::Other(bus, _) => bus,
            _ => me,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum V {
    S(String),
    I(i32),
    U(u32),
    N(i16),
    B(bool),
    D(f64),
    Ref(Ref),
    Refs(Vec<Ref>),
    Strs(Vec<String>),
    Words([u32; 2]),
    Ints(Vec<i32>),
    Dict(Vec<(String, String)>),
    Tuple(Vec<V>),
    Var(Box<V>),
    Actions(Vec<(String, String, String)>),
    Empty(&'static str),
    Props(Vec<(String, V)>),
}

impl V {
    fn sig(&self) -> String {
        match self {
            V::S(_) => "s".into(),
            V::I(_) => "i".into(),
            V::U(_) => "u".into(),
            V::N(_) => "n".into(),
            V::B(_) => "b".into(),
            V::D(_) => "d".into(),
            V::Ref(_) => "(so)".into(),
            V::Refs(_) => "a(so)".into(),
            V::Strs(_) => "as".into(),
            V::Words(_) => "au".into(),
            V::Ints(_) => "ai".into(),
            V::Dict(_) => "a{ss}".into(),
            V::Tuple(vs) => format!("({})", vs.iter().map(V::sig).collect::<String>()),
            V::Var(_) => "v".into(),
            V::Actions(_) => "a(sss)".into(),
            V::Empty(item) => format!("a{item}"),
            V::Props(_) => "a{sv}".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Arg {
    S(String),
    I(i64),
    D(f64),
    L(Vec<Arg>),
}

fn int(a: Option<&Arg>) -> i64 {
    match a {
        Some(Arg::I(i)) => *i,
        Some(Arg::L(v)) => int(v.first()),
        _ => 0,
    }
}

fn word(a: Option<&Arg>) -> &str {
    match a {
        Some(Arg::S(s)) => s,
        _ => "",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ev {
    pub at: Ref,
    pub iface: &'static str,
    pub member: &'static str,
    pub detail: &'static str,
    pub d1: i32,
    pub any: V,
}

impl Ev {
    fn object(at: Ref, member: &'static str, detail: &'static str, d1: i32, any: V) -> Self {
        Ev { at, iface: OBJECT_EVENTS, member, detail, d1, any }
    }
}

fn state_event(at: Ref, detail: &'static str, on: bool) -> Ev {
    Ev::object(at, "StateChanged", detail, on.into(), V::I(0))
}

pub(crate) fn listening() -> bool {
    LISTENING.load(Ordering::Acquire)
}

fn poke() {
    let fd = WAKE.load(Ordering::Acquire);
    if fd >= 0 {
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

pub(crate) fn publish(win: WindowId, origin: (i32, i32), tree: Tree) {
    let mut guard = hub();
    let hub = &mut *guard;
    let mut events = Vec::new();
    match hub.windows.iter_mut().find(|w| w.id == win) {
        Some(w) => {
            changes(win, Some(&w.tree), &tree, &mut events);
            *w = Win { id: win, origin, tree };
        }
        None => {
            events.push(Ev::object(Ref::App, "ChildrenChanged", "add", hub.windows.len() as i32, V::Ref(Ref::Node(win, ROOT_ID))));
            changes(win, None, &tree, &mut events);
            hub.windows.push(Win { id: win, origin, tree });
        }
    }
    if !events.is_empty() {
        hub.events.append(&mut events);
        poke();
    }
}

pub(crate) fn closed(win: WindowId) {
    let mut hub = hub();
    if let Some(i) = hub.windows.iter().position(|w| w.id == win) {
        hub.windows.remove(i);
        hub.events.push(Ev::object(Ref::App, "ChildrenChanged", "remove", i as i32, V::Ref(Ref::Node(win, ROOT_ID))));
        poke();
    }
}

fn selectable(role: Role) -> bool {
    matches!(role, Role::Tab | Role::TreeItem | Role::ListItem | Role::DataItem)
}

fn container(role: Role) -> bool {
    matches!(role, Role::TabList | Role::List | Role::Tree | Role::DataGrid)
}

pub(crate) fn changes(win: WindowId, old: Option<&Tree>, new: &Tree, out: &mut Vec<Ev>) {
    let node = |id| Ref::Node(win, id);
    let mut picked = Vec::new();
    let index: HashMap<u64, usize> = old.map(|t| t.nodes.iter().enumerate().map(|(k, n)| (n.id, k)).collect()).unwrap_or_default();
    for n in &new.nodes {
        let Some(o) = old.zip(index.get(&n.id)).map(|(t, &k)| &t.nodes[k]) else {
            if old.is_some() && n.selected && selectable(n.role) {
                out.push(state_event(node(n.id), "selected", true));
                picked.push(n.parent);
            }
            continue;
        };
        let old = old.expect("an old node comes from an old tree");
        let before: Vec<u64> = o.children.iter().map(|&c| old.nodes[c].id).collect();
        let after: Vec<u64> = n.children.iter().map(|&c| new.nodes[c].id).collect();
        if before != after {
            for (i, id) in before.iter().enumerate().filter(|(_, id)| !after.contains(id)) {
                out.push(Ev::object(node(n.id), "ChildrenChanged", "remove", i as i32, V::Ref(node(*id))));
            }
            for (i, id) in after.iter().enumerate().filter(|(_, id)| !before.contains(id)) {
                out.push(Ev::object(node(n.id), "ChildrenChanged", "add", i as i32, V::Ref(node(*id))));
            }
        }
        if o.name != n.name {
            out.push(Ev::object(node(n.id), "PropertyChange", "accessible-name", 0, V::S(n.name.clone())));
        }
        if o.selected != n.selected {
            if selectable(n.role) {
                out.push(state_event(node(n.id), "selected", n.selected));
                picked.push(n.parent);
            }
            if n.role == Role::CheckBox {
                out.push(state_event(node(n.id), "checked", n.selected));
            }
        }
    }
    picked.sort_unstable();
    picked.dedup();
    for p in picked {
        out.push(Ev::object(node(new.nodes[p].id), "SelectionChanged", "", 0, V::I(0)));
    }
    let focus_of = |t: &Tree| t.focus.map(|k| t.nodes[k].id);
    let (was, now) = (old.and_then(focus_of), focus_of(new));
    if was != now {
        if let Some(id) = was.filter(|&id| new.find(id).is_some()) {
            out.push(state_event(node(id), "focused", false));
        }
        if let Some(id) = now {
            out.push(state_event(node(id), "focused", true));
            out.push(Ev { at: node(id), iface: FOCUS_EVENTS, member: "Focus", detail: "", d1: 0, any: V::I(0) });
        }
    }
}

fn role(t: &Tree, k: usize) -> (u32, &'static str) {
    let n = &t.nodes[k];
    match n.role {
        Role::Window => (23, "frame"),
        Role::Dialog => (16, "dialog"),
        Role::TabList => (38, "page tab list"),
        Role::Tab => (37, "page tab"),
        Role::ToolBar => (63, "tool bar"),
        Role::Button => (43, "push button"),
        Role::CheckBox => (7, "check box"),
        Role::ComboBox => (11, "combo box"),
        Role::Group | Role::Header | Role::Pane => (39, "panel"),
        Role::Link => (88, "link"),
        Role::Edit => (79, "entry"),
        Role::Tree => (65, "tree"),
        Role::TreeItem => (91, "tree item"),
        Role::List => (31, "list"),
        Role::ListItem => (32, "list item"),
        Role::DataGrid => (55, "table"),
        Role::DataItem => (90, "table row"),
        Role::HeaderItem => (10, "column header"),
        Role::Text if k != 0 && t.nodes[n.parent].role == Role::DataItem => (56, "table cell"),
        Role::Text => (29, "label"),
    }
}

fn states(t: &Tree, k: usize) -> [u32; 2] {
    let n = &t.nodes[k];
    let shown = n.bounds.w > 0.0 && n.bounds.h > 0.0;
    let flags = [
        (state::ENABLED, n.enabled),
        (state::SENSITIVE, n.enabled),
        (state::VISIBLE, shown),
        (state::SHOWING, shown),
        (state::FOCUSABLE, n.role != Role::Text),
        (state::FOCUSED, t.focus == Some(k)),
        (state::ACTIVE, k == 0 && t.focus.is_some()),
        (state::SELECTABLE, selectable(n.role)),
        (state::SELECTED, selectable(n.role) && n.selected),
        (state::CHECKABLE, n.role == Role::CheckBox),
        (state::CHECKED, n.role == Role::CheckBox && n.selected),
        (state::EDITABLE, n.role == Role::Edit),
        (state::SINGLE_LINE, n.role == Role::Edit),
        (state::READ_ONLY, n.role == Role::Text),
        (state::MULTISELECTABLE, matches!(n.role, Role::List | Role::DataGrid)),
        (state::MODAL, n.role == Role::Dialog),
    ];
    let bits = flags.iter().filter(|f| f.1).fold(0u64, |b, f| b | 1 << f.0);
    [bits as u32, (bits >> 32) as u32]
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Click,
    Select,
    Toggle,
}

impl Act {
    fn name(self) -> &'static str {
        match self {
            Act::Click => "click",
            Act::Select => "select",
            Act::Toggle => "toggle",
        }
    }

    fn event(self, id: u64) -> Event {
        match self {
            Act::Select => Event::A11ySelect(id),
            Act::Click | Act::Toggle => Event::Invoke(id),
        }
    }
}

fn actions(role: Role) -> &'static [Act] {
    match role {
        Role::CheckBox => &[Act::Toggle],
        Role::Tab | Role::TreeItem | Role::ListItem | Role::DataItem => &[Act::Click, Act::Select],
        Role::Button | Role::ComboBox | Role::HeaderItem | Role::Link => &[Act::Click],
        _ => &[],
    }
}

fn interfaces(role: Role) -> Vec<&'static str> {
    let mut out = vec![ACCESSIBLE, COMPONENT];
    if !actions(role).is_empty() {
        out.push(ACTION);
    }
    if container(role) {
        out.push(SELECTION);
    }
    if role == Role::DataGrid {
        out.push(TABLE);
    }
    if matches!(role, Role::Edit | Role::Text) {
        out.push(TEXT);
    }
    out
}

#[derive(Clone, Copy)]
struct View<'h> {
    hub: &'h Hub,
    win: &'h Win,
    k: usize,
}

impl<'h> View<'h> {
    fn tree(&self) -> &'h Tree {
        &self.win.tree
    }

    fn node(&self) -> &'h Node {
        &self.win.tree.nodes[self.k]
    }

    fn at(&self, k: usize) -> Ref {
        Ref::Node(self.win.id, self.tree().nodes[k].id)
    }

    fn child(&self, i: i64) -> Option<usize> {
        usize::try_from(i).ok().and_then(|i| self.node().children.get(i).copied())
    }

    fn content(&self) -> &'h str {
        let n = self.node();
        if n.role == Role::Edit { &n.value } else { &n.name }
    }

    fn grid(&self) -> (Option<usize>, Vec<usize>) {
        let t = self.tree();
        let kids = &self.node().children;
        (kids.iter().copied().find(|&c| t.nodes[c].role == Role::Header), kids.iter().copied().filter(|&c| t.nodes[c].role == Role::DataItem).collect())
    }

    fn columns(&self) -> i64 {
        self.grid().0.map_or(1, |h| self.tree().nodes[h].children.len().max(1)) as i64
    }

    fn offset(&self, coord: i64) -> (f32, f32) {
        match coord {
            1 => (0.0, 0.0),
            2 if self.k != 0 => {
                let p = self.tree().nodes[self.node().parent].bounds;
                (-p.x, -p.y)
            }
            _ => (self.win.origin.0 as f32, self.win.origin.1 as f32),
        }
    }

    fn rect(&self, coord: i64) -> [i32; 4] {
        let b = self.node().bounds;
        let (dx, dy) = self.offset(coord);
        [(b.x + dx) as i32, (b.y + dy) as i32, b.w as i32, b.h as i32]
    }

    fn local(&self, args: &[Arg]) -> (f32, f32) {
        let (dx, dy) = self.offset(int(args.get(2)));
        (int(args.first()) as f32 - dx, int(args.get(1)) as f32 - dy)
    }

    fn hit(&self, x: f32, y: f32) -> Ref {
        let t = self.tree();
        if !self.node().bounds.contains(x, y) {
            return Ref::Null;
        }
        let mut k = self.k;
        while let Some(&c) = t.nodes[k].children.iter().rev().find(|&&c| t.nodes[c].bounds.contains(x, y)) {
            k = c;
        }
        self.at(k)
    }
}

fn locate<'h>(hub: &'h Hub, r: &Ref) -> Option<View<'h>> {
    let Ref::Node(w, id) = r else { return None };
    let win = hub.windows.iter().find(|x| x.id == *w)?;
    Some(View { hub, win, k: win.tree.find(*id)? })
}

struct Info {
    name: String,
    description: String,
    help: String,
    id: String,
    parent: Ref,
    children: Vec<Ref>,
    index: i32,
    role: (u32, &'static str),
    states: [u32; 2],
    attrs: Vec<(String, String)>,
    interfaces: Vec<&'static str>,
}

fn app_info(hub: &Hub, parent: &Ref) -> Info {
    Info {
        name: TOOLKIT.into(),
        description: String::new(),
        help: String::new(),
        id: String::new(),
        parent: parent.clone(),
        children: hub.windows.iter().map(|w| Ref::Node(w.id, ROOT_ID)).collect(),
        index: -1,
        role: ROLE_APPLICATION,
        states: [0, 0],
        attrs: vec![("toolkit".into(), TOOLKIT.into())],
        interfaces: vec![ACCESSIBLE, APPLICATION],
    }
}

fn node_info(v: View) -> Info {
    let (t, n) = (v.tree(), v.node());
    let index = if v.k == 0 { v.hub.windows.iter().position(|w| w.id == v.win.id) } else { t.nodes[n.parent].children.iter().position(|&c| c == v.k) };
    let mut attrs = vec![("toolkit".to_owned(), TOOLKIT.to_owned())];
    if !n.value.is_empty() {
        attrs.push(("value".into(), n.value.clone()));
    }
    Info {
        name: n.name.clone(),
        description: if n.help.is_empty() { n.value.clone() } else { n.help.clone() },
        help: n.help.clone(),
        id: format!("{:x}", n.id),
        parent: if v.k == 0 { Ref::App } else { v.at(n.parent) },
        children: n.children.iter().map(|&c| v.at(c)).collect(),
        index: index.map_or(-1, |i| i as i32),
        role: role(t, v.k),
        states: states(t, v.k),
        attrs,
        interfaces: interfaces(n.role),
    }
}

type Reply = Result<Vec<V>, &'static str>;

struct Answer<'a> {
    hub: &'a Hub,
    parent: &'a Ref,
    app_id: &'a mut i32,
    act: &'a dyn Fn(WindowId, Event),
}

impl Answer<'_> {
    fn info(&self, at: &Ref) -> Option<Info> {
        match at {
            Ref::App => Some(app_info(self.hub, self.parent)),
            _ => locate(self.hub, at).map(node_info),
        }
    }

    fn call(&mut self, path: &str, iface: &str, member: &str, args: &[Arg]) -> Reply {
        if iface == CACHE && member == "GetItems" {
            return Ok(vec![V::Empty(CACHE_ITEM)]);
        }
        let at = Ref::parse(path).ok_or(UNKNOWN_OBJECT)?;
        let info = self.info(&at).ok_or(UNKNOWN_OBJECT)?;
        if iface == PROPERTIES {
            let target = word(args.first());
            return match member {
                "Get" => self.prop(&at, &info, target, word(args.get(1))).map(|v| vec![V::Var(Box::new(v))]).ok_or(UNKNOWN_PROPERTY),
                "GetAll" => Ok(vec![V::Props(PROPS.iter().filter(|(i, _)| *i == target).filter_map(|(_, p)| Some((p.to_string(), self.prop(&at, &info, target, p)?))).collect())]),
                "Set" if at == Ref::App && target == APPLICATION && word(args.get(1)) == "Id" => {
                    *self.app_id = int(args.get(2)) as i32;
                    Ok(Vec::new())
                }
                _ => Err(UNKNOWN_METHOD),
            };
        }
        if iface == ACCESSIBLE {
            return accessible(&info, member, args);
        }
        if iface == APPLICATION && member == "GetLocale" {
            return Ok(vec![V::S(String::new())]);
        }
        let v = locate(self.hub, &at).ok_or(UNKNOWN_METHOD)?;
        node_call(v, iface, member, args, self.act)
    }

    fn prop(&self, at: &Ref, info: &Info, iface: &str, name: &str) -> Option<V> {
        if !info.interfaces.contains(&iface) {
            return None;
        }
        let view = locate(self.hub, at);
        Some(match (iface, name) {
            (ACCESSIBLE, "Name") => V::S(info.name.clone()),
            (ACCESSIBLE, "Description") => V::S(info.description.clone()),
            (ACCESSIBLE, "Parent") => V::Ref(info.parent.clone()),
            (ACCESSIBLE, "ChildCount") => V::I(info.children.len() as i32),
            (ACCESSIBLE, "Locale") => V::S(String::new()),
            (ACCESSIBLE, "AccessibleId") => V::S(info.id.clone()),
            (ACCESSIBLE, "HelpText") => V::S(info.help.clone()),
            (APPLICATION, "ToolkitName") => V::S(TOOLKIT.into()),
            (APPLICATION, "Version") => V::S(env!("CARGO_PKG_VERSION").into()),
            (APPLICATION, "AtspiVersion") => V::S("2.1".into()),
            (APPLICATION, "Id") => V::I(*self.app_id),
            (ACTION, "NActions") => V::I(actions(view?.node().role).len() as i32),
            (SELECTION, "NSelectedChildren") => {
                let v = view?;
                V::I(v.node().children.iter().filter(|&&c| v.tree().nodes[c].selected).count() as i32)
            }
            (TEXT, "CharacterCount") => V::I(view?.content().chars().count() as i32),
            (TEXT, "CaretOffset") => {
                let v = view?;
                V::I(if v.node().role == Role::Edit { v.content().chars().count() as i32 } else { 0 })
            }
            (TABLE, "NRows") => V::I(view?.grid().1.len() as i32),
            (TABLE, "NColumns") => V::I(view?.columns() as i32),
            (TABLE, "Caption" | "Summary") => V::Ref(Ref::Null),
            (TABLE, "NSelectedRows") => {
                let v = view?;
                V::I(v.grid().1.iter().filter(|&&r| v.tree().nodes[r].selected).count() as i32)
            }
            (TABLE, "NSelectedColumns") => V::I(0),
            _ => return None,
        })
    }
}

fn accessible(info: &Info, member: &str, args: &[Arg]) -> Reply {
    let child = || usize::try_from(int(args.first())).ok().and_then(|i| info.children.get(i).cloned()).unwrap_or(Ref::Null);
    Ok(vec![match member {
        "GetChildAtIndex" => V::Ref(child()),
        "GetChildren" => V::Refs(info.children.clone()),
        "GetIndexInParent" => V::I(info.index),
        "GetRelationSet" => V::Empty("(ua(so))"),
        "GetRole" => V::U(info.role.0),
        "GetRoleName" | "GetLocalizedRoleName" => V::S(info.role.1.into()),
        "GetState" => V::Words(info.states),
        "GetAttributes" => V::Dict(info.attrs.clone()),
        "GetApplication" => V::Ref(Ref::App),
        "GetInterfaces" => V::Strs(info.interfaces.iter().map(|s| s.to_string()).collect()),
        _ => return Err(UNKNOWN_METHOD),
    }])
}

fn node_call(v: View, iface: &str, member: &str, args: &[Arg], act: &dyn Fn(WindowId, Event)) -> Reply {
    let (t, n) = (v.tree(), v.node());
    let first = int(args.first());
    let refer = |k: Option<usize>| V::Ref(k.map_or(Ref::Null, |k| v.at(k)));
    let acts = actions(n.role);
    let action = usize::try_from(first).ok().and_then(|i| acts.get(i).copied());
    let content: Vec<char> = v.content().chars().collect();
    let (header, rows) = if iface == TABLE { v.grid() } else { (None, Vec::new()) };
    let columns = v.columns();
    let row = |r: i64| usize::try_from(r).ok().and_then(|r| rows.get(r).copied());
    let cell = |r: i64, c: i64| row(r).and_then(|k| if c == 0 { Some(k) } else { usize::try_from(c - 1).ok().and_then(|c| t.nodes[k].children.get(c).copied()) });
    let column = |c: i64| header.and_then(|h| usize::try_from(c).ok().and_then(|c| t.nodes[h].children.get(c).copied()));
    let whole = || vec![V::S(content.iter().collect()), V::I(0), V::I(content.len() as i32)];
    Ok(match (iface, member) {
        (COMPONENT, "GetExtents") => vec![V::Tuple(v.rect(first).map(V::I).to_vec())],
        (COMPONENT, "GetPosition") => v.rect(first)[..2].iter().map(|&p| V::I(p)).collect(),
        (COMPONENT, "GetSize") => v.rect(1)[2..].iter().map(|&p| V::I(p)).collect(),
        (COMPONENT, "Contains") => {
            let (x, y) = v.local(args);
            vec![V::B(n.bounds.contains(x, y))]
        }
        (COMPONENT, "GetAccessibleAtPoint") => {
            let (x, y) = v.local(args);
            vec![V::Ref(v.hit(x, y))]
        }
        (COMPONENT, "GrabFocus") => {
            act(v.win.id, Event::A11yFocus(n.id));
            vec![V::B(true)]
        }
        (COMPONENT, "GetLayer") => vec![V::U(if v.k == 0 { LAYER_WINDOW } else { LAYER_WIDGET })],
        (COMPONENT, "GetMDIZOrder") => vec![V::N(0)],
        (COMPONENT, "GetAlpha") => vec![V::D(1.0)],
        (ACTION, "GetName" | "GetLocalizedName") => vec![V::S(action.map_or("", Act::name).into())],
        (ACTION, "GetDescription" | "GetKeyBinding") => vec![V::S(String::new())],
        (ACTION, "GetActions") => vec![V::Actions(acts.iter().map(|a| (a.name().into(), String::new(), String::new())).collect())],
        (ACTION, "DoAction") => {
            if let Some(a) = action {
                act(v.win.id, a.event(n.id));
            }
            vec![V::B(action.is_some())]
        }
        (SELECTION, "GetSelectedChild") => refer(usize::try_from(first).ok().and_then(|i| n.children.iter().copied().filter(|&c| t.nodes[c].selected).nth(i))).into_vec(),
        (SELECTION, "SelectChild") => {
            let pick = v.child(first).filter(|&c| selectable(t.nodes[c].role));
            if let Some(c) = pick {
                act(v.win.id, Event::A11ySelect(t.nodes[c].id));
            }
            vec![V::B(pick.is_some())]
        }
        (SELECTION, "IsChildSelected") => vec![V::B(v.child(first).is_some_and(|c| t.nodes[c].selected))],
        (SELECTION, "DeselectSelectedChild" | "DeselectChild" | "SelectAll" | "ClearSelection") => vec![V::B(false)],
        (TEXT, "GetText") => {
            let end = usize::try_from(int(args.get(1))).unwrap_or(content.len()).min(content.len());
            let start = usize::try_from(first).unwrap_or(0).min(end);
            vec![V::S(content[start..end].iter().collect())]
        }
        (TEXT, "GetCharacterAtOffset") => vec![V::I(usize::try_from(first).ok().and_then(|i| content.get(i)).map_or(0, |&c| c as i32))],
        (TEXT, "GetTextAtOffset" | "GetStringAtOffset") => whole(),
        (TEXT, "GetNSelections") => vec![V::I(0)],
        (TEXT, "GetDefaultAttributes") => vec![V::Dict(Vec::new())],
        (TABLE, "GetAccessibleAt") => refer(cell(first, int(args.get(1)))).into_vec(),
        (TABLE, "GetIndexAt") => vec![V::I((first * columns + int(args.get(1))) as i32)],
        (TABLE, "GetRowAtIndex") => vec![V::I((first / columns) as i32)],
        (TABLE, "GetColumnAtIndex") => vec![V::I((first % columns) as i32)],
        (TABLE, "GetRowDescription") => vec![V::S(row(first).map(|k| t.nodes[k].name.clone()).unwrap_or_default())],
        (TABLE, "GetColumnDescription") => vec![V::S(column(first).map(|k| t.nodes[k].name.clone()).unwrap_or_default())],
        (TABLE, "GetRowExtentAt" | "GetColumnExtentAt") => vec![V::I(1)],
        (TABLE, "GetRowHeader") => vec![V::Ref(Ref::Null)],
        (TABLE, "GetColumnHeader") => refer(column(first)).into_vec(),
        (TABLE, "GetSelectedRows") => vec![V::Ints(rows.iter().enumerate().filter(|(_, &k)| t.nodes[k].selected).map(|(i, _)| i as i32).collect())],
        (TABLE, "GetSelectedColumns") => vec![V::Ints(Vec::new())],
        (TABLE, "IsRowSelected" | "IsSelected") => vec![V::B(row(first).is_some_and(|k| t.nodes[k].selected))],
        (TABLE, "IsColumnSelected") => vec![V::B(false)],
        _ => return Err(UNKNOWN_METHOD),
    })
}

impl V {
    fn into_vec(self) -> Vec<V> {
        vec![self]
    }
}

unsafe fn basic<T>(api: &Api, it: &mut Iter, t: u8, v: T) {
    (api.iter_append_basic)(it, t as c_int, (&v as *const T).cast());
}

unsafe fn string(api: &Api, it: &mut Iter, t: u8, s: &str) {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    basic(api, it, t, c.as_ptr());
}

unsafe fn open(api: &Api, it: &mut Iter, t: u8, sig: Option<&str>, body: impl FnOnce(&mut Iter)) {
    let sig = sig.map(|s| CString::new(s).unwrap_or_default());
    let mut sub = Iter::new();
    (api.open_container)(it, t as c_int, sig.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()), &mut sub);
    body(&mut sub);
    (api.close_container)(it, &mut sub);
}

unsafe fn put(api: &Api, it: &mut Iter, me: &str, v: &V) {
    match v {
        V::S(s) => string(api, it, b's', s),
        V::I(i) => basic(api, it, b'i', *i),
        V::U(u) => basic(api, it, b'u', *u),
        V::N(n) => basic(api, it, b'n', *n),
        V::B(b) => basic(api, it, b'b', u32::from(*b)),
        V::D(d) => basic(api, it, b'd', *d),
        V::Ref(r) => open(api, it, b'r', None, |s| {
            string(api, s, b's', r.bus(me));
            string(api, s, b'o', &r.path());
        }),
        V::Refs(rs) => open(api, it, b'a', Some("(so)"), |s| rs.iter().for_each(|r| put(api, s, me, &V::Ref(r.clone())))),
        V::Strs(v) => open(api, it, b'a', Some("s"), |s| v.iter().for_each(|x| string(api, s, b's', x))),
        V::Words(w) => open(api, it, b'a', Some("u"), |s| w.iter().for_each(|&x| basic(api, s, b'u', x))),
        V::Ints(v) => open(api, it, b'a', Some("i"), |s| v.iter().for_each(|&x| basic(api, s, b'i', x))),
        V::Dict(d) => open(api, it, b'a', Some("{ss}"), |s| {
            for (k, x) in d {
                open(api, s, b'e', None, |e| {
                    string(api, e, b's', k);
                    string(api, e, b's', x);
                });
            }
        }),
        V::Tuple(vs) => open(api, it, b'r', None, |s| vs.iter().for_each(|x| put(api, s, me, x))),
        V::Var(inner) => open(api, it, b'v', Some(&inner.sig()), |s| put(api, s, me, inner)),
        V::Actions(a) => open(api, it, b'a', Some("(sss)"), |s| {
            for (x, y, z) in a {
                open(api, s, b'r', None, |r| [x, y, z].iter().for_each(|w| string(api, r, b's', w)));
            }
        }),
        V::Empty(item) => open(api, it, b'a', Some(item), |_| {}),
        V::Props(p) => open(api, it, b'a', Some("{sv}"), |s| {
            for (k, x) in p {
                open(api, s, b'e', None, |e| {
                    string(api, e, b's', k);
                    put(api, e, me, &V::Var(Box::new(x.clone())));
                });
            }
        }),
    }
}

unsafe fn get<T: Default>(api: &Api, it: &mut Iter) -> T {
    let mut v = T::default();
    (api.iter_basic)(it, (&mut v as *mut T).cast());
    v
}

unsafe fn read(api: &Api, it: &mut Iter) -> Vec<Arg> {
    let mut out = Vec::new();
    loop {
        let t = (api.iter_type)(it);
        if t == 0 {
            return out;
        }
        out.push(match t as u8 {
            b's' | b'o' | b'g' => Arg::S(text(get::<usize>(api, it) as *const c_char)),
            b'i' => Arg::I(get::<i32>(api, it).into()),
            b'u' | b'b' | b'h' => Arg::I(get::<u32>(api, it).into()),
            b'n' => Arg::I(get::<i16>(api, it).into()),
            b'q' => Arg::I(get::<u16>(api, it).into()),
            b'y' => Arg::I(get::<u8>(api, it).into()),
            b'x' | b't' => Arg::I(get::<i64>(api, it)),
            b'd' => Arg::D(get::<f64>(api, it)),
            _ => {
                let mut sub = Iter::new();
                (api.iter_recurse)(it, &mut sub);
                Arg::L(read(api, &mut sub))
            }
        });
        if (api.iter_next)(it) == 0 {
            return out;
        }
    }
}

unsafe fn args(api: &Api, msg: *mut c_void) -> Vec<Arg> {
    let mut it = Iter::new();
    if (api.iter_init)(msg, &mut it) == 0 {
        return Vec::new();
    }
    read(api, &mut it)
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

struct Conn<'a> {
    api: &'a Api,
    raw: *mut c_void,
    me: String,
}

impl<'a> Conn<'a> {
    fn open(api: &'a Api, address: &str) -> Option<Self> {
        let address = CString::new(address).ok()?;
        let mut err = dbus::Error::new();
        unsafe {
            (api.error_init)(&mut err);
            let raw = (api.open_private)(address.as_ptr(), &mut err);
            let mut conn = (!raw.is_null()).then(|| Conn { api, raw, me: String::new() });
            if conn.as_ref().is_some_and(|c| (api.register)(c.raw, &mut err) != 0) {
                if let Some(c) = &mut conn {
                    c.me = text((api.unique_name)(raw));
                }
                return conn;
            }
            tracing::debug!("accessibility bus connection: {}", err.message());
            (api.error_free)(&mut err);
            None
        }
    }

    fn fd(&self) -> c_int {
        let mut fd = -1;
        unsafe { (self.api.unix_fd)(self.raw, &mut fd) };
        fd
    }

    fn pending(&self) -> bool {
        unsafe { (self.api.dispatch_status)(self.raw) == DATA_REMAINS }
    }

    unsafe fn append(&self, msg: *mut c_void, values: &[V]) {
        let mut it = Iter::new();
        (self.api.iter_init_append)(msg, &mut it);
        for v in values {
            put(self.api, &mut it, &self.me, v);
        }
    }

    unsafe fn send(&self, msg: *mut c_void) {
        if !msg.is_null() {
            (self.api.send)(self.raw, msg, std::ptr::null_mut());
            (self.api.unref)(msg);
        }
    }

    fn call(&self, dest: &str, path: &str, iface: &str, member: &str, values: &[V]) -> Option<Vec<Arg>> {
        let api = self.api;
        unsafe {
            let msg = (api.new_method_call)(cstr(dest).as_ptr(), cstr(path).as_ptr(), cstr(iface).as_ptr(), cstr(member).as_ptr());
            if msg.is_null() {
                return None;
            }
            self.append(msg, values);
            let mut err = dbus::Error::new();
            (api.error_init)(&mut err);
            let reply = (api.call)(self.raw, msg, CALL_TIMEOUT_MS, &mut err);
            (api.unref)(msg);
            if reply.is_null() {
                tracing::debug!("{iface}.{member}: {}", err.message());
                (api.error_free)(&mut err);
                return None;
            }
            let out = args(api, reply);
            (api.unref)(reply);
            Some(out)
        }
    }

    fn add_match(&self, rule: &str) {
        let mut err = dbus::Error::new();
        unsafe {
            (self.api.error_init)(&mut err);
            (self.api.add_match)(self.raw, cstr(rule).as_ptr(), &mut err);
            (self.api.error_free)(&mut err);
        }
    }

    fn drain(&self, mut each: impl FnMut(*mut c_void)) -> bool {
        unsafe {
            if (self.api.read_write)(self.raw, 0) == 0 {
                return false;
            }
            loop {
                let msg = (self.api.pop)(self.raw);
                if msg.is_null() {
                    return true;
                }
                each(msg);
                (self.api.unref)(msg);
            }
        }
    }

    fn emit(&self, e: &Ev) {
        unsafe {
            let msg = (self.api.new_signal)(cstr(&e.at.path()).as_ptr(), cstr(e.iface).as_ptr(), cstr(e.member).as_ptr());
            if msg.is_null() {
                return;
            }
            self.append(msg, &[V::S(e.detail.into()), V::I(e.d1), V::I(0), V::Var(Box::new(e.any.clone())), V::Props(Vec::new())]);
            self.send(msg);
        }
    }

    fn flush(&self) {
        unsafe { (self.api.flush)(self.raw) };
    }
}

impl Drop for Conn<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.raw);
            (self.api.release)(self.raw);
        }
    }
}

struct Bridge<'a> {
    conn: Conn<'a>,
    parent: Ref,
    app_id: i32,
}

impl<'a> Bridge<'a> {
    fn connect(api: &'a Api, session: &Conn) -> Option<Self> {
        let reply = session.call(BUS_NAME, BUS_PATH, BUS_NAME, "GetAddress", &[])?;
        let conn = Conn::open(api, word(reply.first()))?;
        let reply = conn.call(REGISTRY, ROOT_PATH, "org.a11y.atspi.Socket", "Embed", &[V::Ref(Ref::App)])?;
        let parent = match reply.first() {
            Some(Arg::L(p)) => match p.as_slice() {
                [Arg::S(bus), Arg::S(path)] => Ref::Other(bus.clone(), path.clone()),
                _ => Ref::Null,
            },
            _ => Ref::Null,
        };
        Some(Bridge { conn, parent, app_id: 0 })
    }

    fn pump(&mut self, act: &dyn Fn(WindowId, Event)) -> bool {
        let Bridge { conn, parent, app_id } = self;
        let api = conn.api;
        let alive = conn.drain(|msg| unsafe {
            if (api.msg_type)(msg) != TYPE_METHOD_CALL {
                return;
            }
            let (path, iface, member) = (text((api.path)(msg)), text((api.interface)(msg)), text((api.member)(msg)));
            let args = args(api, msg);
            let reply = Answer { hub: &hub(), parent: &*parent, app_id: &mut *app_id, act }.call(&path, &iface, &member, &args);
            match reply {
                Ok(values) => {
                    let r = (api.new_return)(msg);
                    if !r.is_null() {
                        conn.append(r, &values);
                    }
                    conn.send(r);
                }
                Err(name) => conn.send((api.new_error)(msg, cstr(name).as_ptr(), cstr(&format!("{iface}.{member}")).as_ptr())),
            }
        });
        let events = std::mem::take(&mut hub().events);
        for e in &events {
            conn.emit(e);
        }
        conn.flush();
        alive
    }
}

fn status(session: &Conn) -> Option<bool> {
    let reply = session.call(BUS_NAME, BUS_PATH, PROPERTIES, "GetAll", &[V::S("org.a11y.Status".into())])?;
    let Some(Arg::L(entries)) = reply.first() else { return Some(false) };
    Some(entries.iter().any(|e| matches!(e, Arg::L(kv) if matches!(kv.as_slice(), [Arg::S(k), v] if (k == "IsEnabled" || k == "ScreenReaderEnabled") && int(Some(v)) != 0))))
}

fn forget() {
    LISTENING.store(false, Ordering::Release);
    let mut hub = hub();
    hub.windows.clear();
    hub.events.clear();
}

fn watch(fd: c_int) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}

pub(crate) fn run(address: &str, refresh: &dyn Fn(), act: &dyn Fn(WindowId, Event)) {
    let Some(api) = dbus::load() else { return };
    let Some(session) = Conn::open(&api, address) else { return };
    session.add_match(STATUS_MATCH);
    let Some(mut wanted) = status(&session) else { return };
    let mut pipe = [-1; 2];
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return;
    }
    WAKE.store(pipe[1], Ordering::Release);
    let mut bridge: Option<Bridge> = None;
    loop {
        if wanted && bridge.is_none() {
            bridge = Bridge::connect(&api, &session);
            wanted = bridge.is_some();
            if wanted {
                LISTENING.store(true, Ordering::Release);
                refresh();
            }
        }
        if !wanted && bridge.take().is_some() {
            forget();
        }
        let mut changed = false;
        let alive = session.drain(|msg| unsafe { changed |= (api.msg_type)(msg) == TYPE_SIGNAL && text((api.member)(msg)) == "PropertiesChanged" });
        if !alive {
            break;
        }
        if changed {
            wanted = status(&session).unwrap_or(false);
            continue;
        }
        if bridge.as_mut().is_some_and(|b| !b.pump(act)) {
            bridge = None;
            wanted = false;
            forget();
            continue;
        }
        if session.pending() || bridge.as_ref().is_some_and(|b| b.conn.pending()) {
            continue;
        }
        let mut fds = vec![watch(session.fd()), watch(pipe[0])];
        fds.extend(bridge.as_ref().map(|b| watch(b.conn.fd())));
        unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1);
            let mut sink = [0u8; 64];
            while libc::read(pipe[0], sink.as_mut_ptr().cast(), sink.len()) > 0 {}
        }
    }
    WAKE.store(-1, Ordering::Release);
    drop(bridge);
    forget();
    unsafe {
        libc::close(pipe[0]);
        libc::close(pipe[1]);
    }
}

pub(crate) fn start(refresh: impl Fn() + Send + 'static, act: impl Fn(WindowId, Event) + Send + 'static) {
    let Some(address) = dbus::session_address() else { return };
    let spawned = std::thread::Builder::new().name("flit-atspi".into()).spawn(move || run(&address, &refresh, &act));
    if let Err(e) = spawned {
        tracing::warn!("cannot start the accessibility bridge: {e}");
    }
}

#[cfg(test)]
#[path = "../test/atspi.rs"]
mod tests;
