use super::{text, Bus};
use crate::atspi::{args, Arg, V};
use flit_platform::tray::{Action, Menu, Tray};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DBUS: &CStr = c"org.freedesktop.DBus";
const WATCHER: &CStr = c"org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &CStr = c"/StatusNotifierWatcher";
const ITEM: &CStr = c"org.kde.StatusNotifierItem";
const ITEM_PATH: &CStr = c"/StatusNotifierItem";
const MENU: &CStr = c"com.canonical.dbusmenu";
const MENU_PATH: &CStr = c"/Menu";
const STATUS_XML: &str = "<node><interface name=\"org.kde.StatusNotifierItem\">\
<method name=\"Activate\"><arg type=\"i\"/><arg type=\"i\"/></method>\
<method name=\"ProvideXdgActivationToken\"><arg type=\"s\"/></method>\
<method name=\"SecondaryActivate\"><arg type=\"i\"/><arg type=\"i\"/></method>\
<method name=\"ContextMenu\"><arg type=\"i\"/><arg type=\"i\"/></method>\
<method name=\"Scroll\"><arg type=\"i\"/><arg type=\"s\"/></method>\
<property name=\"Category\" type=\"s\" access=\"read\"/><property name=\"Id\" type=\"s\" access=\"read\"/>\
<property name=\"Title\" type=\"s\" access=\"read\"/><property name=\"Status\" type=\"s\" access=\"read\"/>\
<property name=\"IconName\" type=\"s\" access=\"read\"/><property name=\"IconPixmap\" type=\"a(iiay)\" access=\"read\"/>\
<property name=\"OverlayIconName\" type=\"s\" access=\"read\"/><property name=\"OverlayIconPixmap\" type=\"a(iiay)\" access=\"read\"/>\
<property name=\"AttentionIconName\" type=\"s\" access=\"read\"/><property name=\"AttentionIconPixmap\" type=\"a(iiay)\" access=\"read\"/>\
<property name=\"ToolTip\" type=\"(sa(iiay)ss)\" access=\"read\"/>\
<property name=\"ItemIsMenu\" type=\"b\" access=\"read\"/><property name=\"Menu\" type=\"o\" access=\"read\"/>\
<signal name=\"NewStatus\"><arg type=\"s\"/></signal></interface></node>";
const MENU_XML: &str = "<node><interface name=\"com.canonical.dbusmenu\">\
<method name=\"GetLayout\"><arg type=\"i\"/><arg type=\"i\"/><arg type=\"as\"/><arg direction=\"out\" type=\"u\"/><arg direction=\"out\" type=\"(ia{sv}av)\"/></method>\
<method name=\"Event\"><arg type=\"i\"/><arg type=\"s\"/><arg type=\"v\"/><arg type=\"u\"/></method>\
<method name=\"AboutToShow\"><arg type=\"i\"/><arg direction=\"out\" type=\"b\"/></method>\
<method name=\"GetGroupProperties\"><arg type=\"ai\"/><arg type=\"as\"/><arg direction=\"out\" type=\"a(ia{sv})\"/></method>\
<method name=\"GetProperty\"><arg type=\"i\"/><arg type=\"s\"/><arg direction=\"out\" type=\"v\"/></method>\
<method name=\"EventGroup\"><arg type=\"a(isvu)\"/><arg direction=\"out\" type=\"ai\"/></method>\
<method name=\"AboutToShowGroup\"><arg type=\"ai\"/><arg direction=\"out\" type=\"ai\"/><arg direction=\"out\" type=\"ai\"/></method>\
<property name=\"Version\" type=\"u\" access=\"read\"/><property name=\"Status\" type=\"s\" access=\"read\"/>\
<property name=\"TextDirection\" type=\"s\" access=\"read\"/><property name=\"IconThemePath\" type=\"as\" access=\"read\"/>\
<signal name=\"LayoutUpdated\"><arg type=\"u\"/><arg type=\"i\"/></signal></interface></node>";

struct Item {
    id: i32,
    label: Option<String>,
    action: Option<Action>,
    children: Vec<Item>,
}

impl Item {
    fn command(id: i32, label: String, action: Action) -> Self {
        Self { id, label: Some(label), action: Some(action), children: Vec::new() }
    }

    fn tree(menu: Option<&Menu>) -> Self {
        let children = menu.map(|menu| {
            let mut children = vec![Self::command(1, menu.new_window.clone(), Action::NewWindow)];
            if !menu.pinned.is_empty() {
                let pinned = menu.pinned.iter().enumerate().map(|(i, (loc, label))| Self::command(i as i32 + 4, label.clone(), Action::Bookmark(loc.clone()))).collect();
                children.push(Self { id: 3, label: Some(menu.bookmarks.clone()), action: None, children: pinned });
            }
            children.push(Self::command(2, menu.quit.clone(), Action::Quit));
            children
        }).unwrap_or_default();
        Self { id: 0, label: None, action: None, children }
    }

    fn find(&self, id: i32) -> Option<&Self> {
        if id == self.id { Some(self) } else { self.children.iter().find_map(|n| n.find(id)) }
    }

    fn props(&self, names: &[Arg]) -> Vec<(String, V)> {
        let mut props = Vec::new();
        if let Some(label) = &self.label {
            props.push(("label".to_owned(), V::S(label.replace('_', "__"))));
        }
        if !self.children.is_empty() && self.id != 0 {
            props.push(("children-display".to_owned(), V::S("submenu".into())));
        }
        props.retain(|(key, _)| names.is_empty() || names.contains(&Arg::S(key.clone())));
        props
    }

    fn layout(&self, depth: i32, names: &[Arg]) -> V {
        let children = if depth == 0 { Vec::new() } else { self.children.iter().map(|n| V::Var(Box::new(n.layout(depth.saturating_sub(1), names)))).collect() };
        V::Tuple(vec![V::I(self.id), V::Props(self.props(names)), V::Array("v", children)])
    }

    fn clicked(&self, id: i32, event: &str) -> Option<Action> {
        if event == "clicked" { self.find(id)?.action.clone() } else { None }
    }
}

fn properties(item: bool, active: bool) -> Vec<(String, V)> {
    let vals = if item {
        vec![
            ("Category", V::S("ApplicationStatus".into())),
            ("Id", V::S("flit".into())),
            ("Title", V::S("Flit".into())),
            ("Status", status(active)),
            ("IconName", V::S("flit".into())),
            ("IconPixmap", V::Empty("(iiay)")),
            ("OverlayIconName", V::S(String::new())),
            ("OverlayIconPixmap", V::Empty("(iiay)")),
            ("AttentionIconName", V::S(String::new())),
            ("AttentionIconPixmap", V::Empty("(iiay)")),
            ("ToolTip", V::Tuple(vec![V::S("flit".into()), V::Empty("(iiay)"), V::S("Flit".into()), V::S(String::new())])),
            ("ItemIsMenu", V::B(false)),
            ("Menu", V::O(MENU_PATH.to_string_lossy().into_owned())),
        ]
    } else {
        vec![("Version", V::U(3)), ("TextDirection", V::S("ltr".into())), ("Status", V::S("normal".into())), ("IconThemePath", V::Empty("s"))]
    };
    vals.into_iter().map(|(key, value)| (key.into(), value)).collect()
}

fn status(active: bool) -> V {
    V::S(if active { "Active" } else { "Passive" }.into())
}

#[derive(Default)]
struct Activation(Option<(String, String, Instant)>);

impl Activation {
    fn provide(&mut self, sender: String, token: String) {
        self.0 = Some((sender, token, Instant::now()));
    }

    fn send(&mut self, sender: String, tray: &Tray, action: Action) {
        if action != Action::Quit {
            if let Some((_, token, _)) = self.0.take_if(|(owner, _, at)| *owner == sender && at.elapsed() < Duration::from_secs(5)) {
                crate::window::startup_token(token);
            }
        }
        tray.send(action);
    }
}


enum Event<'a> {
    Token(&'a str),
    Action(Action),
}

struct Export<'a> {
    tree: &'a Item,
    revision: u32,
    active: bool,
}

impl Export<'_> {
    fn reply(&self, path: &CStr, iface: &CStr, member: &CStr, values: &[Arg], mut emit: impl FnMut(Event<'_>)) -> Result<Vec<V>, &'static CStr> {
        if path != ITEM_PATH && path != MENU_PATH {
            return Err(c"org.freedesktop.DBus.Error.UnknownObject");
        }
        let item = path == ITEM_PATH;
        let tree = self.tree;
        let active = self.active;
        let reply = match (iface.to_bytes(), member.to_bytes(), values) {
            (b"org.freedesktop.DBus.Introspectable", b"Introspect", []) => Some(vec![V::S(if item { STATUS_XML } else { MENU_XML }.into())]),
            (b"org.freedesktop.DBus.Properties", b"GetAll", [Arg::S(name)]) if name.as_bytes() == (if item { ITEM } else { MENU }).to_bytes() => {
                Some(vec![V::Props(properties(item, active))])
            }
            (b"org.freedesktop.DBus.Properties", b"Get", [Arg::S(name), Arg::S(key)]) if name.as_bytes() == (if item { ITEM } else { MENU }).to_bytes() => {
                properties(item, active).into_iter().find(|(k, _)| k == key).map(|(_, value)| vec![V::Var(Box::new(value))])
            }
            (b"org.kde.StatusNotifierItem", b"ProvideXdgActivationToken", [Arg::S(token)]) if item => {
                if active { emit(Event::Token(token)); }
                Some(Vec::new())
            }
            (b"org.kde.StatusNotifierItem", b"Activate" | b"SecondaryActivate", [Arg::I(_), Arg::I(_)]) if item => {
                if active { emit(Event::Action(Action::Activate)); }
                Some(Vec::new())
            }
            (b"org.kde.StatusNotifierItem", b"ContextMenu", [Arg::I(_), Arg::I(_)])
            | (b"org.kde.StatusNotifierItem", b"Scroll", [Arg::I(_), Arg::S(_)]) if item => Some(Vec::new()),
            (b"com.canonical.dbusmenu", b"GetLayout", [Arg::I(id), Arg::I(depth), Arg::L(names)]) if !item => {
                i32::try_from(*id).ok().and_then(|id| tree.find(id)).map(|n| vec![V::U(self.revision), n.layout(*depth as i32, names)])
            }
            (b"com.canonical.dbusmenu", b"GetGroupProperties", [Arg::L(ids), Arg::L(names)]) if !item => {
                let props = ids.iter().filter_map(|id| {
                    let Arg::I(id) = id else { return None };
                    let n = tree.find(*id as i32)?;
                    Some(V::Tuple(vec![V::I(n.id), V::Props(n.props(names))]))
                }).collect();
                Some(vec![V::Array("(ia{sv})", props)])
            }
            (b"com.canonical.dbusmenu", b"GetProperty", [Arg::I(id), Arg::S(name)]) if !item => {
                tree.find(*id as i32).and_then(|n| n.props(&[Arg::S(name.clone())]).pop()).map(|(_, v)| vec![V::Var(Box::new(v))])
            }
            (b"com.canonical.dbusmenu", b"AboutToShow", [Arg::I(_)]) if !item => Some(vec![V::B(false)]),
            (b"com.canonical.dbusmenu", b"AboutToShowGroup", [Arg::L(_)]) if !item => Some(vec![V::Empty("i"), V::Empty("i")]),
            (b"com.canonical.dbusmenu", b"Event", [Arg::I(id), Arg::S(event), _, Arg::I(_)]) if !item => {
                if let Some(action) = tree.clicked(*id as i32, event) { emit(Event::Action(action)); }
                Some(Vec::new())
            }
            (b"com.canonical.dbusmenu", b"EventGroup", [Arg::L(events)]) if !item => {
                for ev in events {
                    if let Arg::L(fields) = ev {
                        if let [Arg::I(id), Arg::S(event), _, Arg::I(_)] = fields.as_slice() {
                            if let Some(action) = tree.clicked(*id as i32, event) { emit(Event::Action(action)); }
                        }
                    }
                }
                Some(vec![V::Empty("i")])
            }
            _ => None,
        };
        reply.ok_or(c"org.freedesktop.DBus.Error.UnknownMethod")
    }
}

unsafe fn handle(bus: &Bus, msg: *mut c_void, tray: &Tray, export: &Export<'_>, activation: &mut Activation) {
    let api = &bus.api;
    let header = |get: unsafe extern "C" fn(*mut c_void) -> *const c_char| {
        let value = get(msg);
        if value.is_null() { c"" } else { CStr::from_ptr(value) }
    };
    let reply = export.reply(header(api.path), header(api.interface), header(api.member), &args(api, msg), |event| {
        let sender = text((api.sender)(msg));
        match event {
            Event::Token(token) => activation.provide(sender, token.to_owned()),
            Event::Action(action) => activation.send(sender, tray, action),
        }
    });
    match reply {
        Ok(reply) => bus.respond(msg, &reply),
        Err(name) => {
            let error = (api.new_error)(msg, name.as_ptr(), c"Unknown tray request".as_ptr());
            bus.send(error, &[]);
        }
    }
}

fn watcher(bus: &Bus) -> Option<CString> {
    let name = V::S(WATCHER.to_string_lossy().into_owned());
    let values = bus.call(DBUS, c"/org/freedesktop/DBus", DBUS, c"GetNameOwner", &[name])?;
    let [Arg::S(owner)] = values.as_slice() else { return None };
    CString::new(owner.as_str()).ok()
}

fn has_host(bus: &Bus, owner: Option<&CStr>) -> bool {
    let Some(dest) = owner else { return false };
    let args = [V::S(WATCHER.to_string_lossy().into_owned()), V::S("IsStatusNotifierHostRegistered".into())];
    bus.call(dest, WATCHER_PATH, c"org.freedesktop.DBus.Properties", c"Get", &args)
        .is_some_and(|value| matches!(value.as_slice(), [Arg::L(inner)] if matches!(inner.as_slice(), [Arg::I(1)])))
}


const HOST_GRACE: Duration = Duration::from_secs(30);
const WATCH_READABLE: u32 = 1;
const WATCH_WRITABLE: u32 = 2;
const WATCH_ERROR: u32 = 4;
const WATCH_HANGUP: u32 = 8;
const DATA_REMAINS: i32 = 0;

#[derive(Default)]
struct HostAvailability(Option<Instant>);

impl HostAvailability {
    fn update(&mut self, registered: bool, now: Instant) -> (Option<bool>, Option<Duration>) {
        if registered {
            self.0 = None;
            return (Some(true), None);
        }
        let deadline = *self.0.get_or_insert(now + HOST_GRACE);
        let remaining = deadline.checked_duration_since(now).filter(|duration| !duration.is_zero());
        (remaining.is_none().then_some(false), remaining)
    }
}

#[derive(Default)]
struct Registration {
    owner: Option<CString>,
    registered: bool,
}

impl Registration {
    fn refresh(&mut self, owner: Option<CString>, host: bool, register: impl FnOnce(&CStr) -> bool) {
        if owner != self.owner || !host {
            self.registered = false;
        }
        self.owner = owner;
        if host && !self.registered {
            self.registered = self.owner.as_deref().is_some_and(register);
        }
    }
}

struct Wakeup {
    event: OwnedFd,
    active: Mutex<bool>,
}

impl Wakeup {
    fn new() -> Option<Self> {
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        (fd >= 0).then(|| Self {
            event: unsafe { OwnedFd::from_raw_fd(fd) },
            active: Mutex::new(true),
        })
    }

    fn poll_fd(&self) -> libc::pollfd {
        libc::pollfd { fd: self.event.as_raw_fd(), events: libc::POLLIN, revents: 0 }
    }

    fn notify(&self) {
        let event = 1u64;
        while unsafe { libc::write(self.event.as_raw_fd(), std::ptr::from_ref(&event).cast(), size_of::<u64>()) } < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
    }

    fn drain(&self) {
        let mut event = 0u64;
        while unsafe { libc::read(self.event.as_raw_fd(), std::ptr::from_mut(&mut event).cast(), size_of::<u64>()) } < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
    }

    fn running(&self) -> bool {
        *self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(&self, tray: &Tray, available: Option<bool>) {
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if *active {
            tray.set_availability(available);
        }
    }

    fn stop(&self) {
        *self.active.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.notify();
    }
}

type WatchList = RefCell<Vec<*mut c_void>>;

unsafe extern "C" fn add_watch(watch: *mut c_void, data: *mut c_void) -> u32 {
    (&*data.cast::<WatchList>()).borrow_mut().push(watch);
    1
}

unsafe extern "C" fn remove_watch(watch: *mut c_void, data: *mut c_void) {
    (&*data.cast::<WatchList>()).borrow_mut().retain(|&entry| entry != watch);
}

struct Watches<'a> {
    bus: &'a Bus,
    list: Box<WatchList>,
}

impl<'a> Watches<'a> {
    fn new(bus: &'a Bus) -> Option<Self> {
        let list = Box::<WatchList>::default();
        let data = std::ptr::from_ref(list.as_ref()).cast_mut().cast();
        let ok = unsafe { (bus.api.set_watches)(bus.conn, Some(add_watch), Some(remove_watch), None, data, None) != 0 };
        if !ok {
            return None;
        }
        Some(Self { bus, list })
    }

    fn wait(&self, wakeup: &Wakeup, timeout: Option<Duration>) -> bool {
        let api = &self.bus.api;
        let watches: Vec<_> = self.list.borrow().iter().copied().filter(|&watch| unsafe { (api.watch_enabled)(watch) != 0 }).collect();
        let mut fds = vec![wakeup.poll_fd()];
        fds.extend(watches.iter().map(|&watch| unsafe {
            let flags = (api.watch_flags)(watch);
            let mut events = 0;
            if flags & WATCH_READABLE != 0 { events |= libc::POLLIN; }
            if flags & WATCH_WRITABLE != 0 { events |= libc::POLLOUT; }
            libc::pollfd { fd: (api.watch_fd)(watch), events, revents: 0 }
        }));
        let millis = timeout.map_or(-1, |duration| duration.as_nanos().div_ceil(1_000_000) as i32);
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
        if ready < 0 {
            return std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
        }
        if fds[0].revents & libc::POLLIN != 0 {
            wakeup.drain();
            if !wakeup.running() {
                return false;
            }
        }
        for (watch, fd) in watches.into_iter().zip(&fds[1..]) {
            if fd.revents == 0 || !self.list.borrow().contains(&watch) {
                continue;
            }
            let mut flags = 0;
            if fd.revents & libc::POLLIN != 0 { flags |= WATCH_READABLE; }
            if fd.revents & libc::POLLOUT != 0 { flags |= WATCH_WRITABLE; }
            if fd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 { flags |= WATCH_ERROR; }
            if fd.revents & libc::POLLHUP != 0 { flags |= WATCH_HANGUP; }
            unsafe {
                if (api.watch_enabled)(watch) != 0 && (api.watch_handle)(watch, flags) == 0 {
                    return false;
                }
            }
        }
        true
    }
}

impl Drop for Watches<'_> {
    fn drop(&mut self) {
        unsafe { (self.bus.api.set_watches)(self.bus.conn, None, None, None, std::ptr::null_mut(), None) };
    }
}

pub fn host_available() -> bool {
    Bus::session().is_some_and(|bus| has_host(&bus, watcher(&bus).as_deref()))
}

fn serve(tray: &Tray, wakeup: &Wakeup) -> Option<()> {
    let bus = Bus::session()?;
    let watches = Watches::new(&bus)?;
    bus.subscribe(c"type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.kde.StatusNotifierWatcher'");
    bus.subscribe(c"type='signal',sender='org.kde.StatusNotifierWatcher',path='/StatusNotifierWatcher',interface='org.kde.StatusNotifierWatcher'");
    bus.subscribe(c"type='signal',sender='org.kde.StatusNotifierWatcher',path='/StatusNotifierWatcher',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged',arg0='org.kde.StatusNotifierWatcher'");
    let api = &bus.api;
    let mut registration = Registration::default();
    let mut availability = HostAvailability::default();
    let mut refresh = true;
    let mut menu = tray.menu()?;
    let mut tree = Item::tree(Some(&menu));
    let mut revision = 1u32;
    let mut activation = Activation::default();
    while wakeup.running() {
        let next = tray.menu()?;
        if next != menu {
            menu = next;
            tree = Item::tree(Some(&menu));
            revision = revision.wrapping_add(1);
            if registration.registered {
                unsafe { bus.signal(MENU_PATH, MENU, c"LayoutUpdated", &[V::U(revision), V::I(0)]) };
            }
        }
        let export = Export { tree: &tree, revision, active: true };
        unsafe {
            loop {
                let msg = (api.pop)(bus.conn);
                if msg.is_null() {
                    break;
                }
                match (api.msg_type)(msg) {
                    1 => handle(&bus, msg, tray, &export, &mut activation),
                    4 => {
                        let sender = (api.sender)(msg);
                        let sender = (!sender.is_null()).then(|| CStr::from_ptr(sender));
                        let iface = text((api.interface)(msg));
                        let member = text((api.member)(msg));
                        if sender == Some(DBUS) && iface == "org.freedesktop.DBus" && member == "NameOwnerChanged" {
                            refresh = true;
                        } else if sender.is_some() && sender == registration.owner.as_deref() && text((api.path)(msg)) == "/StatusNotifierWatcher" {
                            if iface == "org.kde.StatusNotifierWatcher" && matches!(member.as_str(), "StatusNotifierHostRegistered" | "StatusNotifierHostUnregistered") {
                                if member == "StatusNotifierHostUnregistered" {
                                    registration.registered = false;
                                }
                                refresh = true;
                            } else if iface == "org.freedesktop.DBus.Properties" && member == "PropertiesChanged" {
                                refresh = true;
                            }
                        }
                    }
                    _ => {}
                }
                (api.unref)(msg);
            }
            if (api.is_connected)(bus.conn) == 0 {
                break;
            }
        }
        if std::mem::take(&mut refresh) {
            let owner = watcher(&bus);
            let host = has_host(&bus, owner.as_deref());
            registration.refresh(owner, host, |dest| {
                bus.call(dest, WATCHER_PATH, WATCHER, c"RegisterStatusNotifierItem", &[V::S("/StatusNotifierItem".into())]).is_some()
            });
        }
        let (available, timeout) = availability.update(registration.registered, Instant::now());
        wakeup.publish(tray, available);
        if unsafe { (api.dispatch_status)(bus.conn) } != DATA_REMAINS && !watches.wait(wakeup, timeout) {
            break;
        }
    }
    Some(())
}

fn start_worker(tray: &Tray) -> Option<Arc<Wakeup>> {
    let wakeup = Arc::new(Wakeup::new()?);
    let worker = wakeup.clone();
    let tray = tray.clone();
    std::thread::Builder::new().name("flit-tray".into()).spawn(move || {
        let _ = serve(&tray, &worker);
        worker.publish(&tray, Some(false));
    }).ok()?;
    Some(wakeup)
}

pub(crate) struct TrayListener(Tray);

impl Drop for TrayListener {
    fn drop(&mut self) {
        self.0.set_menu_listener(None);
    }
}

pub(crate) fn start(tray: Tray) -> TrayListener {
    listen(tray, start_worker)
}

fn listen(tray: Tray, start: impl Fn(&Tray) -> Option<Arc<Wakeup>> + Send + Sync + 'static) -> TrayListener {
    let worker = Mutex::<Option<Arc<Wakeup>>>::new(None);
    tray.set_menu_listener(Some(Arc::new(move |tray, enabled| {
        let mut worker = worker.lock().unwrap_or_else(|e| e.into_inner());
        if enabled {
            if let Some(worker) = &*worker {
                worker.notify();
            } else {
                *worker = start(tray);
                if worker.is_none() {
                    tray.set_available(false);
                }
            }
        } else {
            if let Some(worker) = worker.take() {
                worker.stop();
            }
            tray.set_availability(None);
        }
    })));
    TrayListener(tray)
}

#[cfg(test)]
#[path = "../test/tray.rs"]
mod tests;
