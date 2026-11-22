use crate::atspi::{args, Arg, V};
use crate::dbus::{text, Bus};
use flit_core::pick::{valid_name, Choice, Choices, Filter, Mode, Request};
use flit_core::FsPath;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CString};
use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const FRONTEND: &str = "org.freedesktop.portal.Desktop";
const NAME: &std::ffi::CStr = c"org.freedesktop.impl.portal.desktop.flit";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.impl.portal.Request";
pub(crate) const PARENT_ENV: &str = "FLIT_FILE_CHOOSER_PARENT";
pub(crate) const MODAL_ENV: &str = "FLIT_FILE_CHOOSER_MODAL";
const REQUEST_XML: &str = "<node><interface name=\"org.freedesktop.impl.portal.Request\"><method name=\"Close\"/><signal name=\"Created\"/><property name=\"version\" type=\"u\" access=\"read\"/></interface></node>";
const METHOD: &str = "<arg type=\"o\"/><arg type=\"s\"/><arg type=\"s\"/><arg type=\"s\"/><arg type=\"a{sv}\"/><arg direction=\"out\" type=\"u\"/><arg direction=\"out\" type=\"a{sv}\"/>";

type FilterRules = (String, Vec<(u32, String)>);
type Picked = (Vec<Vec<u8>>, usize, Choices);

struct Call {
    request: Request,
    filters: Vec<FilterRules>,
    modal: bool,
}

fn option<'a>(options: &'a [Arg], name: &str) -> Option<&'a Arg> {
    options.iter().rev().find_map(|option| {
        let Arg::L(entry) = option else { return None };
        match entry.as_slice() {
            [Arg::S(key), Arg::L(value)] if key == name => value.first(),
            _ => None,
        }
    })
}

fn string(value: Option<&Arg>) -> Option<&str> {
    match value? {
        Arg::S(s) => Some(s),
        _ => None,
    }
}

fn list(value: Option<&Arg>) -> &[Arg] {
    match value {
        Some(Arg::L(v)) => v,
        _ => &[],
    }
}

fn bytes(value: &Arg) -> Option<Vec<u8>> {
    let Arg::L(values) = value else { return None };
    let (Arg::I(0), bytes) = values.split_last()? else { return None };
    bytes
        .iter()
        .map(|v| match v {
            Arg::I(i) if *i > 0 => u8::try_from(*i).ok(),
            _ => None,
        })
        .collect()
}

fn filter(value: &Arg) -> Option<FilterRules> {
    let Arg::L(entry) = value else { return None };
    let [Arg::S(label), Arg::L(raw)] = entry.as_slice() else { return None };
    let rules: Vec<_> = raw
        .iter()
        .filter_map(|value| {
            let Arg::L(rule) = value else { return None };
            match rule.as_slice() {
                [Arg::I(kind @ (0 | 1)), Arg::S(text)] if !text.is_empty() => Some((*kind as u32, text.clone())),
                _ => None,
            }
        })
        .collect();
    (!rules.is_empty()).then(|| (label.clone(), rules))
}

fn choice(value: &Arg) -> Option<Choice> {
    let Arg::L(fields) = value else { return None };
    let [Arg::S(id), Arg::S(label), Arg::L(items), Arg::S(selected)] = fields.as_slice() else { return None };
    let values = if items.is_empty() {
        vec![("false".to_owned(), String::new()), ("true".to_owned(), String::new())]
    } else {
        items
            .iter()
            .filter_map(|item| {
                let Arg::L(fields) = item else { return None };
                match fields.as_slice() {
                    [Arg::S(value), Arg::S(label)] => Some((value.clone(), title_label(label))),
                    _ => None,
                }
            })
            .collect()
    };
    if !values.is_empty() {
        let index = values.iter().position(|(value, _)| value == selected).unwrap_or(0);
        Some(Choice { id: id.clone(), label: title_label(label), values, selected: index })
    } else {
        None
    }
}

fn title_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '_' || chars.next_if_eq(&'_').is_some() {
            out.push(ch);
        }
    }
    out
}

fn parse(method: &str, title: &str, options: &[Arg], resolve_mime: impl Fn(String) -> Option<String>) -> Option<Call> {
    let flag = |key| option(options, key) == Some(&Arg::I(1));
    let folder = option(options, "current_folder").and_then(bytes).filter(|p| p.starts_with(b"/"));
    let mut path = folder;
    let mode = match method {
        "OpenFile" => Mode::Open { multiple: flag("multiple"), directories: flag("directory") },
        "SaveFile" => {
            let current_file = option(options, "current_file").and_then(bytes).filter(|p| p.starts_with(b"/"));
            let suggested = current_file.as_deref().and_then(|p| p.rsplit(|&b| b == b'/').next()).filter(|n| valid_name(n, true));
            let name = string(option(options, "current_name")).map(str::as_bytes).filter(|n| valid_name(n, true)).or(suggested).unwrap_or_default();
            if let Some(file) = current_file.as_deref().and_then(FsPath::parse_exact) {
                if let Some(parent) = file.parent() {
                    path = Some(parent.as_bytes().to_vec());
                }
            }
            Mode::Save(name.to_vec())
        }
        "SaveFiles" => {
            let names: Vec<_> = list(option(options, "files")).iter().map(bytes).collect::<Option<_>>()?;
            if names.is_empty() || names.iter().any(|name| !valid_name(name, true)) {
                return None;
            }
            Mode::SaveMany(names)
        }
        _ => return None,
    };
    let mut filters: Vec<_> = list(option(options, "filters")).iter().filter_map(filter).collect();
    let current = option(options, "current_filter").and_then(filter);
    if filters.is_empty() {
        filters.extend(current.iter().cloned());
    }
    let filter = current.and_then(|c| filters.iter().position(|f| *f == c)).unwrap_or(0);
    let entries = filters
        .iter()
        .map(|(label, rules)| {
            let (mut patterns, mut mime_types) = (Vec::new(), Vec::new());
            for (kind, value) in rules {
                let value = value.to_lowercase();
                match kind {
                    0 => patterns.push(value),
                    1 if value == "*/*" => patterns.push("*".into()),
                    1 => mime_types.extend(resolve_mime(value)),
                    _ => {}
                }
            }
            patterns.sort_unstable();
            patterns.dedup();
            if patterns.iter().any(|pattern| pattern == "*") {
                mime_types.clear();
            } else {
                mime_types.sort_unstable();
                mime_types.dedup();
            }
            Filter { label: label.clone(), patterns, mime_types }
        })
        .collect();
    let title = (!title.is_empty()).then(|| title.to_owned());
    let accept = string(option(options, "accept_label")).filter(|s| !s.is_empty()).map(title_label);
    let choices = list(option(options, "choices")).iter().filter_map(choice).collect();
    let modal = option(options, "modal") != Some(&Arg::I(0));
    Some(Call { request: Request { mode, path, title, accept, filters: entries, filter, choices }, filters, modal })
}

pub fn picker_mime(path: &FsPath) -> Option<std::sync::Arc<[String]>> {
    crate::mime::picker_mime(flit_platform_linux::host(path)?)
}

fn choose_names(dir: &FsPath, names: &[Vec<u8>], mut exists: impl FnMut(&FsPath) -> io::Result<bool>) -> Option<Vec<FsPath>> {
    let mut used = HashSet::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let split = name.iter().rposition(|&b| b == b'.').filter(|&i| i != 0).unwrap_or(name.len());
        let (stem, ext) = name.split_at(split);
        let mut available = None;
        for i in 0..10000 {
            let path = if i == 0 {
                dir.join(name)
            } else {
                let mut replacement = stem.to_vec();
                replacement.extend_from_slice(format!(" ({i})").as_bytes());
                replacement.extend_from_slice(ext);
                dir.join(&replacement)
            };
            if !used.contains(&path) && !exists(&path).ok()? {
                available = Some(path);
                break;
            }
        }
        let path = available?;
        used.insert(path.clone());
        out.push(path);
    }
    Some(out)
}

fn response(code: u32) -> Vec<V> {
    vec![V::U(code), V::Props(Vec::new())]
}

fn results(call: &Call, picked: Picked, exists: impl FnMut(&FsPath) -> io::Result<bool>) -> Vec<V> {
    let (raw, selected, choices) = picked;
    let mut paths: Vec<_> = raw.iter().filter(|p| p.starts_with(b"/") && !p.contains(&0)).filter_map(|p| FsPath::parse_exact(p)).collect();
    let required = match &call.request.mode {
        Mode::Open { multiple: true, .. } => None,
        Mode::SaveMany(names) => {
            let Some(list) = paths.first().and_then(|dir| choose_names(dir, names, exists)) else { return response(2) };
            paths = list;
            Some(names.len())
        }
        _ => Some(1),
    };
    if paths.is_empty() || required.is_some_and(|n| n != paths.len()) {
        return response(2);
    }
    let mut props = vec![("uris".into(), V::Strs(crate::clipboard::uris(&paths)))];
    if let Some((label, rules)) = call.filters.get(selected) {
        let rules = rules.iter().map(|(kind, text)| V::Tuple(vec![V::U(*kind), V::S(text.clone())])).collect();
        props.push(("current_filter".into(), V::Tuple(vec![V::S(label.clone()), V::Array("(us)", rules)])));
    }
    if !choices.is_empty() {
        let pairs = choices.into_iter().map(|(id, value)| V::Tuple(vec![V::S(id), V::S(value)])).collect();
        props.push(("choices".into(), V::Array("(ss)", pairs)));
    }
    vec![V::U(0), V::Props(props)]
}

struct Pending {
    msg: *mut c_void,
    call: Call,
    child: Child,
    output: Receiver<Option<Picked>>,
}

impl Pending {
    fn spawn(msg: *mut c_void, call: Call, parent: &str) -> io::Result<Self> {
        let input = serde_json::to_vec(&call.request)?;
        if input.len() >= 1 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "file chooser options exceed the limit"));
        }
        let mut child = Command::new(std::env::current_exe()?).arg("--file-chooser-request").env(PARENT_ENV, parent).env(MODAL_ENV, if call.modal { "1" } else { "0" }).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("missing file chooser pipes"));
        };
        let (tx, output) = mpsc::channel();
        if let Err(e) = std::thread::Builder::new().name("flit-chooser".into()).spawn(move || {
            let wrote = stdin.write_all(&input).is_ok();
            drop(stdin);
            let mut bytes = Vec::new();
            let read = stdout.take(32 << 20).read_to_end(&mut bytes).is_ok();
            let result = if wrote && read { serde_json::from_slice(&bytes).ok() } else { None };
            let _ = tx.send(result);
        }) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        Ok(Self { msg, call, child, output })
    }

    fn poll(&mut self) -> Option<Vec<V>> {
        let status = match self.child.try_wait() {
            Ok(status) => status?,
            Err(_) => return Some(response(2)),
        };
        if status.code() == Some(1) {
            return Some(response(1));
        }
        if !status.success() {
            return Some(response(2));
        }
        match self.output.try_recv() {
            Ok(Some(picked)) => Some(results(&self.call, picked, |path| match std::fs::symlink_metadata(flit_platform::std_path(path)) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            })),
            Err(mpsc::TryRecvError::Empty) => None,
            _ => Some(response(2)),
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn frontend(bus: &Bus) -> Option<String> {
    let args = bus.call(c"org.freedesktop.DBus", c"/org/freedesktop/DBus", c"org.freedesktop.DBus", c"GetNameOwner", &[V::S(FRONTEND.into())])?;
    match args.as_slice() {
        [Arg::S(owner)] => Some(owner.clone()),
        _ => None,
    }
}

fn valid_handle(path: &str) -> bool {
    path.strip_prefix("/org/freedesktop/portal/desktop/request/").is_some_and(|tail| tail.split('/').all(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')))
}

unsafe fn error(bus: &Bus, msg: *mut c_void, name: &std::ffi::CStr, description: &std::ffi::CStr) {
    bus.send((bus.api.new_error)(msg, name.as_ptr(), description.as_ptr()), &[]);
}

unsafe fn handle(bus: &Bus, msg: *mut c_void, owner: Option<&str>, pending: &mut HashMap<String, Pending>) -> bool {
    let api = bus.api;
    let member = text((api.member)(msg));
    let iface = text((api.interface)(msg));
    let path = text((api.path)(msg));
    if iface == "org.freedesktop.DBus.Introspectable" && member == "Introspect" && path == PATH {
        let methods: String = ["OpenFile", "SaveFile", "SaveFiles"].iter().map(|method| format!("<method name=\"{method}\">{METHOD}</method>")).collect();
        bus.respond(msg, &[V::S(format!("<node><interface name=\"{IFACE}\">{methods}</interface></node>"))]);
        return false;
    }
    if owner != Some(text((api.sender)(msg)).as_str()) {
        error(bus, msg, c"org.freedesktop.DBus.Error.AccessDenied", c"Only the desktop portal can request files");
        return false;
    }
    if pending.contains_key(&path) {
        if iface == "org.freedesktop.DBus.Introspectable" && member == "Introspect" {
            bus.respond(msg, &[V::S(REQUEST_XML.into())]);
            return false;
        }
        if iface == "org.freedesktop.DBus.Properties" {
            let incoming = args(api, msg);
            if let [Arg::S(interface), Arg::S(key)] = incoming.as_slice() {
                if member == "Get" && interface == REQUEST && key == "version" {
                    bus.respond(msg, &[V::Var(Box::new(V::U(2)))]);
                    return false;
                }
            }
            if let [Arg::S(interface)] = incoming.as_slice() {
                if member == "GetAll" && interface == REQUEST {
                    bus.respond(msg, &[V::Props(vec![("version".into(), V::U(2))])]);
                    return false;
                }
            }
        }
    }
    if iface == REQUEST && member == "Close" {
        if let Some(request) = pending.remove(&path) {
            bus.respond(request.msg, &response(1));
            (api.unref)(request.msg);
            bus.respond(msg, &[]);
        } else {
            error(bus, msg, c"org.freedesktop.DBus.Error.UnknownObject", c"File chooser request is no longer active");
        }
        return false;
    }
    if iface == IFACE && path == PATH {
        let incoming = args(api, msg);
        if let [Arg::S(handle), Arg::S(_), Arg::S(parent), Arg::S(title), Arg::L(options)] = incoming.as_slice() {
            if valid_handle(handle) && pending.len() < 16 && !pending.contains_key(handle) {
                if let (Some(call), Ok(path)) = (parse(&member, title, options, crate::mime::picker_filter_type), CString::new(handle.as_bytes())) {
                    if let Ok(request) = Pending::spawn(msg, call, parent) {
                        pending.insert(handle.clone(), request);
                        bus.signal(&path, c"org.freedesktop.impl.portal.Request", c"Created", &[]);
                        return true;
                    }
                }
            }
        }
        bus.respond(msg, &response(2));
    } else {
        error(bus, msg, c"org.freedesktop.DBus.Error.UnknownMethod", c"Unknown file chooser request");
    }
    false
}

fn serve_until_idle(idle: Duration, mut release: impl FnMut(), mut step: impl FnMut(i32) -> Option<(bool, bool)>) {
    let mut used = Instant::now();
    let mut active = false;
    let mut released = false;
    loop {
        let timeout = if active {
            80
        } else if released {
            break;
        } else if used.elapsed() >= idle {
            release();
            released = true;
            0
        } else {
            1000
        };
        let Some((pending, activity)) = step(timeout) else { break };
        active = pending;
        if activity {
            used = Instant::now();
        }
    }
}

pub fn run() -> i32 {
    let Some(bus) = Bus::session() else { return 2 };
    bus.subscribe(c"type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.freedesktop.portal.Desktop'");
    if !bus.own(NAME) {
        return 2;
    }
    let mut owner = frontend(&bus);
    let mut pending = HashMap::new();
    let api = bus.api;
    let release = || {
        let reply = bus.call(c"org.freedesktop.DBus", c"/org/freedesktop/DBus", c"org.freedesktop.DBus", c"ReleaseName", &[V::S(NAME.to_string_lossy().into_owned())]);
        if reply.as_deref() != Some(&[Arg::I(1)]) {
            tracing::warn!("could not release file chooser bus name");
        }
    };
    unsafe {
        serve_until_idle(Duration::from_secs(60), release, |timeout| {
            if (api.read_write)(bus.conn, timeout) == 0 {
                return None;
            }
            let mut activity = false;
            loop {
                let msg = (api.pop)(bus.conn);
                if msg.is_null() {
                    break;
                }
                match (api.msg_type)(msg) {
                    1 => {
                        activity = true;
                        if handle(&bus, msg, owner.as_deref(), &mut pending) {
                            continue;
                        }
                    }
                    4 if text((api.interface)(msg)) == "org.freedesktop.DBus" && text((api.member)(msg)) == "NameOwnerChanged" => {
                        let next = frontend(&bus);
                        if next != owner {
                            for (_, request) in pending.drain() {
                                (api.unref)(request.msg);
                            }
                            owner = next;
                        }
                    }
                    _ => {}
                }
                (api.unref)(msg);
            }
            pending.retain(|_, request| {
                let Some(reply) = request.poll() else { return true };
                activity = true;
                bus.respond(request.msg, &reply);
                (api.unref)(request.msg);
                false
            });
            Some((!pending.is_empty(), activity))
        });
        (api.flush)(bus.conn);
        for (_, request) in pending {
            (api.unref)(request.msg);
        }
    }
    0
}

#[cfg(test)]
#[path = "../test/file_chooser.rs"]
mod tests;
