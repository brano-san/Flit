use crate::atspi::{args, put, Arg, V};
use crate::dbus::{self, text, Api, Iter};
use flit_core::i18n::S;
use std::ffi::{c_int, c_void, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const UDISKS: &str = "org.freedesktop.UDisks2";
const MANAGER: &str = "/org/freedesktop/UDisks2";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const QUERY_MS: c_int = 10_000;
const ACTION_MS: c_int = 300_000;
const TYPE_SIGNAL: c_int = 4;
const DATA_REMAINS: c_int = 0;
pub const SETTLE: Duration = Duration::from_millis(400);
pub const DEVICE_RULES: [&str; 2] = ["type='signal',sender='org.freedesktop.UDisks2',interface='org.freedesktop.DBus.ObjectManager'", "type='signal',sender='org.freedesktop.UDisks2',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"];
pub const GVFS_RULES: [&str; 2] = ["type='signal',interface='org.gtk.vfs.MountTracker'", "type='signal',interface='org.gtk.Private.RemoteVolumeMonitor'"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    pub object: String,
    pub device: Vec<u8>,
    pub preferred: Vec<u8>,
    pub symlinks: Vec<Vec<u8>>,
    pub label: String,
    pub fs_type: String,
    pub drive: String,
    pub backing: String,
    pub mount_points: Vec<Vec<u8>>,
    pub filesystem: bool,
    pub encrypted: bool,
    pub ignore: bool,
    pub system: bool,
}

impl Block {
    fn named(&self, device: &[u8]) -> bool {
        self.device == device || self.preferred == device || self.symlinks.iter().any(|s| s == device)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    pub object: String,
    pub ejectable: bool,
    pub can_power_off: bool,
    pub optical: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub blocks: Vec<Block>,
    pub disks: Vec<Disk>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Unmount(String),
    Lock(String),
    Eject(String),
    PowerOff(String),
}

impl Step {
    fn call(&self) -> (&str, &str, &str) {
        match self {
            Step::Unmount(o) => (o, FILESYSTEM, "Unmount"),
            Step::Lock(o) => (o, ENCRYPTED, "Lock"),
            Step::Eject(o) => (o, DRIVE, "Eject"),
            Step::PowerOff(o) => (o, DRIVE, "PowerOff"),
        }
    }

    fn of_drive(&self) -> bool {
        matches!(self, Step::Eject(_) | Step::PowerOff(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub name: String,
    pub message: String,
}

impl Failure {
    fn new(name: &str, message: impl Into<String>) -> Self {
        Failure { name: name.to_owned(), message: message.into() }
    }

    pub fn busy(&self) -> bool {
        self.name.ends_with(".DeviceBusy") || self.message.ends_with("target is busy")
    }

    pub fn cancelled(&self) -> bool {
        self.name.ends_with(".NotAuthorizedDismissed") || self.name.ends_with(".Cancelled")
    }

    pub fn wrong_passphrase(&self) -> bool {
        if !self.name.ends_with(".Error.Failed") {
            return false;
        }
        let m = self.message.to_ascii_lowercase();
        ["passphrase", "no key available", "not permitted"].iter().any(|w| m.contains(w))
    }

    pub fn reason(&self) -> String {
        match () {
            _ if self.busy() => S::ErrorsFsDriveBusy.en().to_owned(),
            _ if self.name.ends_with(".ServiceUnknown") || self.name.ends_with(".NoServer") || self.name.ends_with(".FileNotFound") => S::ErrorsFsNoUdisks.en().to_owned(),
            _ => self.message.trim().to_owned(),
        }
    }
}

impl From<Failure> for io::Error {
    fn from(f: Failure) -> io::Error {
        let kind = match () {
            _ if f.cancelled() => io::ErrorKind::Interrupted,
            _ if f.busy() => io::ErrorKind::ResourceBusy,
            _ if f.name.ends_with(".NotAuthorized") || f.name.ends_with(".NotAuthorizedCanObtain") => io::ErrorKind::PermissionDenied,
            _ if f.name.ends_with(".FileNotFound") || f.name.ends_with(".UnknownObject") => io::ErrorKind::NotFound,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, f.reason())
    }
}

fn items(a: Option<&Arg>) -> &[Arg] {
    match a {
        Some(Arg::L(v)) => v,
        _ => &[],
    }
}

fn pair(a: &Arg) -> Option<(&str, &Arg)> {
    match a {
        Arg::L(kv) => match kv.as_slice() {
            [Arg::S(k), v] => Some((k, v)),
            _ => None,
        },
        _ => None,
    }
}

fn find<'a>(dict: &'a Arg, name: &str) -> Option<&'a Arg> {
    items(Some(dict)).iter().filter_map(pair).find(|(k, _)| *k == name).map(|(_, v)| v)
}

fn prop<'a>(props: &'a Arg, name: &str) -> Option<&'a Arg> {
    match find(props, name)? {
        Arg::L(inner) if inner.len() == 1 => inner.first(),
        other => Some(other),
    }
}

fn string(a: Option<&Arg>) -> String {
    match a {
        Some(Arg::S(s)) => s.clone(),
        _ => String::new(),
    }
}

fn flag(a: Option<&Arg>) -> bool {
    matches!(a, Some(Arg::I(v)) if *v != 0)
}

fn bytes(a: Option<&Arg>) -> Vec<u8> {
    let mut out: Vec<u8> = items(a).iter().filter_map(|b| if let Arg::I(v) = b { u8::try_from(*v).ok() } else { None }).collect();
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

fn byte_lists(a: Option<&Arg>) -> Vec<Vec<u8>> {
    items(a).iter().map(|x| bytes(Some(x))).filter(|b| !b.is_empty()).collect()
}

pub fn devices(reply: &[Arg]) -> Devices {
    let mut out = Devices::default();
    for (object, ifaces) in items(reply.first()).iter().filter_map(pair) {
        if let Some(b) = find(ifaces, BLOCK) {
            let fs = find(ifaces, FILESYSTEM);
            out.blocks.push(Block {
                object: object.to_owned(),
                device: bytes(prop(b, "Device")),
                preferred: bytes(prop(b, "PreferredDevice")),
                symlinks: byte_lists(prop(b, "Symlinks")),
                label: string(prop(b, "IdLabel")),
                fs_type: string(prop(b, "IdType")),
                drive: string(prop(b, "Drive")),
                backing: string(prop(b, "CryptoBackingDevice")),
                mount_points: fs.map(|f| byte_lists(prop(f, "MountPoints"))).unwrap_or_default(),
                filesystem: fs.is_some(),
                encrypted: find(ifaces, ENCRYPTED).is_some(),
                ignore: flag(prop(b, "HintIgnore")),
                system: flag(prop(b, "HintSystem")),
            });
        }
        if let Some(d) = find(ifaces, DRIVE) {
            out.disks.push(Disk { object: object.to_owned(), ejectable: flag(prop(d, "Ejectable")), can_power_off: flag(prop(d, "CanPowerOff")), optical: flag(prop(d, "Optical")) });
        }
    }
    out
}

fn real(o: &str) -> bool {
    !o.is_empty() && o != "/"
}

impl Devices {
    fn block(&self, object: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.object == object)
    }

    fn mounted_at(&self, point: &[u8]) -> Option<&Block> {
        self.blocks.iter().find(|b| b.mount_points.iter().any(|p| p == point))
    }

    pub fn by_device(&self, device: &[u8]) -> Option<&Block> {
        self.blocks.iter().find(|b| b.named(device))
    }

    pub fn cleartext(&self, encrypted: &Block) -> Option<&Block> {
        self.blocks.iter().find(|b| b.backing == encrypted.object)
    }

    fn release(&self, b: &Block, steps: &mut Vec<Step>) {
        if let Some(clear) = self.cleartext(b) {
            self.release(clear, steps);
        }
        if !b.mount_points.is_empty() {
            steps.push(Step::Unmount(b.object.clone()));
        }
        if b.encrypted && self.cleartext(b).is_some() {
            steps.push(Step::Lock(b.object.clone()));
        }
    }

    pub fn unmount_plan(&self, point: &[u8]) -> Option<Vec<Step>> {
        let b = self.mounted_at(point)?;
        let mut steps = vec![Step::Unmount(b.object.clone())];
        if let Some(outer) = self.block(&b.backing).filter(|o| o.encrypted) {
            steps.push(Step::Lock(outer.object.clone()));
        }
        Some(steps)
    }

    pub fn eject_plan(&self, point: &[u8]) -> Option<Vec<Step>> {
        let b = self.mounted_at(point)?;
        let outer = self.block(&b.backing).unwrap_or(b);
        let Some(disk) = self.disks.iter().find(|d| real(&outer.drive) && d.object == outer.drive) else { return self.unmount_plan(point) };
        let mut steps = Vec::new();
        for part in self.blocks.iter().filter(|p| p.drive == disk.object) {
            self.release(part, &mut steps);
        }
        if disk.optical || (disk.ejectable && !disk.can_power_off) {
            steps.push(Step::Eject(disk.object.clone()));
        } else if disk.can_power_off {
            steps.push(Step::PowerOff(disk.object.clone()));
        }
        Some(steps)
    }

    pub fn locked(&self, b: &Block) -> bool {
        b.encrypted && self.cleartext(b).is_none()
    }
}

pub fn relevant(member: &str, args: &[Arg]) -> bool {
    match member {
        "InterfacesAdded" | "InterfacesRemoved" => true,
        "PropertiesChanged" => matches!(args.first(), Some(Arg::S(i)) if [BLOCK, FILESYSTEM, ENCRYPTED, DRIVE].contains(&i.as_str())),
        "Mounted" | "Unmounted" | "VolumeAdded" | "VolumeRemoved" | "MountAdded" | "MountRemoved" | "DriveConnected" | "DriveDisconnected" => true,
        _ => false,
    }
}

pub fn system_address() -> String {
    std::env::var("DBUS_SYSTEM_BUS_ADDRESS").ok().filter(|a| !a.is_empty()).unwrap_or_else(|| "unix:path=/var/run/dbus/system_bus_socket".to_owned())
}

pub(crate) struct Bus {
    api: Api,
    raw: *mut c_void,
}

unsafe impl Send for Bus {}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

impl Bus {
    pub(crate) fn open(address: &str) -> Result<Bus, Failure> {
        let api = dbus::load().ok_or_else(|| Failure::new("org.freedesktop.DBus.Error.ServiceUnknown", "libdbus-1 is not installed"))?;
        let addr = cstr(address);
        let mut err = dbus::Error::new();
        unsafe {
            (api.error_init)(&mut err);
            let raw = (api.open_private)(addr.as_ptr(), &mut err);
            if raw.is_null() {
                let f = Failure::new("org.freedesktop.DBus.Error.NoServer", err.message());
                (api.error_free)(&mut err);
                return Err(f);
            }
            let bus = Bus { api, raw };
            if (bus.api.register)(raw, &mut err) == 0 {
                let f = Failure::new(&err.name(), err.message());
                (bus.api.error_free)(&mut err);
                return Err(f);
            }
            Ok(bus)
        }
    }

    pub(crate) fn call(&self, dest: &str, path: &str, iface: &str, member: &str, values: &[V], timeout: c_int) -> Result<Vec<Arg>, Failure> {
        let api = &self.api;
        unsafe {
            let msg = (api.new_method_call)(cstr(dest).as_ptr(), cstr(path).as_ptr(), cstr(iface).as_ptr(), cstr(member).as_ptr());
            if msg.is_null() {
                return Err(Failure::new("org.freedesktop.DBus.Error.NoMemory", "cannot build a D-Bus call"));
            }
            let mut it = Iter::new();
            (api.iter_init_append)(msg, &mut it);
            for v in values {
                put(api, &mut it, "", v);
            }
            let mut err = dbus::Error::new();
            (api.error_init)(&mut err);
            let reply = (api.call)(self.raw, msg, timeout, &mut err);
            (api.unref)(msg);
            if reply.is_null() {
                let f = Failure::new(&err.name(), err.message());
                (api.error_free)(&mut err);
                return Err(f);
            }
            let out = args(api, reply);
            (api.unref)(reply);
            Ok(out)
        }
    }

    fn listen(&self, rules: &[&str], settle: Duration, fire: &dyn Fn()) {
        let api = &self.api;
        for rule in rules {
            let mut err = dbus::Error::new();
            unsafe {
                (api.error_init)(&mut err);
                (api.add_match)(self.raw, cstr(rule).as_ptr(), &mut err);
                (api.error_free)(&mut err);
            }
        }
        let mut due: Option<Instant> = None;
        loop {
            let wait = due.map_or(-1, |d| d.saturating_duration_since(Instant::now()).as_millis().min(c_int::MAX as u128) as c_int);
            if unsafe { (api.read_write)(self.raw, wait) } == 0 {
                return;
            }
            loop {
                let msg = unsafe { (api.pop)(self.raw) };
                if msg.is_null() {
                    break;
                }
                unsafe {
                    if (api.msg_type)(msg) == TYPE_SIGNAL && relevant(&text((api.member)(msg)), &args(api, msg)) {
                        due.get_or_insert_with(|| Instant::now() + settle);
                    }
                    (api.unref)(msg);
                }
            }
            if due.is_some_and(|d| Instant::now() >= d) && unsafe { (api.dispatch_status)(self.raw) } != DATA_REMAINS {
                due = None;
                fire();
            }
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.raw);
            (self.api.release)(self.raw);
        }
    }
}

static SYSTEM: Mutex<Option<Bus>> = Mutex::new(None);

fn on_system<T>(f: impl Fn(&Bus) -> Result<T, Failure>) -> Result<T, Failure> {
    let mut held = SYSTEM.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..2 {
        if held.is_none() {
            *held = Some(Bus::open(&system_address())?);
        }
        let Some(bus) = held.as_ref() else { break };
        match f(bus) {
            Err(e) if e.name.ends_with(".Disconnected") || e.name.ends_with(".NoReply") => *held = None,
            done => return done,
        }
    }
    Err(Failure::new("org.freedesktop.DBus.Error.Disconnected", "the system bus connection was lost"))
}

fn options() -> V {
    V::Props(Vec::new())
}

pub fn snapshot() -> Result<Devices, Failure> {
    on_system(|bus| bus.call(UDISKS, MANAGER, OBJECT_MANAGER, "GetManagedObjects", &[], QUERY_MS)).map(|r| devices(&r))
}

pub fn run(steps: &[Step], act: &mut dyn FnMut(&Step) -> Result<(), Failure>) -> Result<(), Failure> {
    for step in steps {
        match act(step) {
            Err(e) if step.of_drive() && !e.cancelled() => tracing::warn!("the drive is unmounted but stays connected: {}", e.message),
            Err(e) => return Err(e),
            Ok(()) => {}
        }
    }
    Ok(())
}

fn perform(step: &Step) -> Result<(), Failure> {
    let (object, iface, member) = step.call();
    on_system(|bus| bus.call(UDISKS, object, iface, member, &[options()], ACTION_MS)).map(drop)
}

fn not_found(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, what.to_owned())
}

pub fn eject(point: &Path) -> io::Result<()> {
    let steps = snapshot()?.eject_plan(point.as_os_str().as_bytes()).ok_or_else(|| not_found("not a mounted drive"))?;
    Ok(run(&steps, &mut perform)?)
}

pub fn unmount(point: &Path) -> io::Result<()> {
    let steps = snapshot()?.unmount_plan(point.as_os_str().as_bytes()).ok_or_else(|| not_found("not a mounted drive"))?;
    Ok(run(&steps, &mut perform)?)
}

fn mount_object(object: &str) -> Result<PathBuf, Failure> {
    let reply = on_system(|bus| bus.call(UDISKS, object, FILESYSTEM, "Mount", &[options()], ACTION_MS))?;
    match reply.first() {
        Some(Arg::S(p)) if !p.is_empty() => Ok(PathBuf::from(p)),
        _ => Err(Failure::new("org.freedesktop.UDisks2.Error.Failed", "mounted, but the mount point is unknown")),
    }
}

pub fn mount(device: &Path) -> io::Result<PathBuf> {
    let devices = snapshot()?;
    let b = devices.by_device(device.as_os_str().as_bytes()).ok_or_else(|| not_found("the device is gone"))?;
    if devices.locked(b) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, S::ErrorsFsLocked.en()));
    }
    let target = devices.cleartext(b).unwrap_or(b);
    if let Some(point) = target.mount_points.first() {
        return Ok(PathBuf::from(OsStr::from_bytes(point)));
    }
    Ok(mount_object(&target.object)?)
}

#[derive(Debug)]
pub struct WrongPassphrase;

impl std::fmt::Display for WrongPassphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(S::ErrorsFsWrongPassphrase.en())
    }
}

impl std::error::Error for WrongPassphrase {}

pub fn is_wrong_passphrase(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<WrongPassphrase>())
}

pub fn unlock_error(f: Failure) -> io::Error {
    if f.wrong_passphrase() && !f.cancelled() {
        io::Error::new(io::ErrorKind::PermissionDenied, WrongPassphrase)
    } else {
        f.into()
    }
}

pub fn unlock(device: &Path, passphrase: &str) -> io::Result<PathBuf> {
    let devices = snapshot()?;
    let b = devices.by_device(device.as_os_str().as_bytes()).ok_or_else(|| not_found("the device is gone"))?;
    let clear = match devices.cleartext(b) {
        Some(c) => c.object.clone(),
        None => {
            let unlocked = on_system(|bus| bus.call(UDISKS, &b.object, ENCRYPTED, "Unlock", &[V::S(passphrase.to_owned()), options()], ACTION_MS));
            string(unlocked.map_err(unlock_error)?.first())
        }
    };
    Ok(mount_object(&clear)?)
}

fn listen_on(name: &str, address: String, rules: &'static [&'static str], fire: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new().name(name.into()).spawn(move || match Bus::open(&address) {
        Ok(bus) => bus.listen(rules, SETTLE, &fire),
        Err(e) => tracing::info!("not watching devices on {address}: {}", e.message),
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot watch devices: {e}");
    }
}

pub fn watch(changed: impl Fn() + Clone + Send + 'static) {
    listen_on("flit-udisks", system_address(), &DEVICE_RULES, changed.clone());
    if let Some(session) = dbus::session_address() {
        listen_on("flit-gvfs", session, &GVFS_RULES, changed);
    }
}

#[cfg(test)]
#[path = "../test/udisks.rs"]
mod tests;
