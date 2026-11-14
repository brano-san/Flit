use flit_core::preview::straight_rgba;
use flit_core::ui::Bitmap;
use flit_core::FsPath;
use std::sync::OnceLock;
use x11rb::connection::{Connection, RequestConnection};
use std::ops::Range;
use x11rb::protocol::xproto::{Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, CreateWindowAux, EventMask, Property, PropMode, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass, SELECTION_NOTIFY_EVENT};
use x11rb::protocol::Event;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

pub const GNOME: &str = "x-special/gnome-copied-files";
pub const KDE_CUT: &str = "application/x-kde-cutselection";
pub const URI_LIST: &str = "text/uri-list";
pub const TEXT: &str = "text/plain;charset=utf-8";
pub const PLAIN: &str = "text/plain";
pub const MARKER: &str = "application/x-flit-files";
pub const PNG: &str = "image/png";
pub const IMAGE_MARKER: &str = "application/x-flit-image";
const INCR_CHUNK: usize = 1 << 18;
pub const FILE_TYPES: [&str; 3] = [GNOME, KDE_CUT, URI_LIST];
const X11_TEXT: [&str; 3] = ["UTF8_STRING", "STRING", "TEXT"];

pub type Payload = Vec<(String, Vec<u8>)>;

fn uris(paths: &[FsPath]) -> Vec<String> {
    paths.iter().map(|p| format!("file://{}", flit_platform_linux::percent_encode(p.as_bytes()))).collect()
}

pub fn gnome(paths: &[FsPath], cut: bool) -> Vec<u8> {
    format!("{}\n{}", if cut { "cut" } else { "copy" }, uris(paths).join("\n")).into_bytes()
}

pub fn uri_list(paths: &[FsPath]) -> Vec<u8> {
    uris(paths).iter().map(|u| format!("{u}\r\n")).collect::<String>().into_bytes()
}

pub fn plain(paths: &[FsPath]) -> Vec<u8> {
    paths.iter().map(FsPath::as_bytes).collect::<Vec<_>>().join(&b'\n')
}

pub fn payload(paths: &[FsPath], cut: bool) -> Payload {
    let text = plain(paths);
    [(GNOME, gnome(paths, cut)), (KDE_CUT, if cut { b"1".to_vec() } else { b"0".to_vec() }), (URI_LIST, uri_list(paths)), (TEXT, text.clone()), (PLAIN, text)].into_iter().map(|(t, b)| (t.to_owned(), b)).collect()
}

pub fn png(image: &Bitmap) -> Option<Vec<u8>> {
    if image.width == 0 || image.height == 0 || image.rgba.len() != image.width as usize * image.height as usize * 4 {
        return None;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&straight_rgba(&image.rgba)).ok()?;
    writer.finish().ok()?;
    Some(out)
}

pub fn files(types: &[(String, Vec<u8>)]) -> Option<(Vec<FsPath>, bool)> {
    let get = |mime: &str| types.iter().find(|(t, b)| t == mime && !b.is_empty()).map(|(_, b)| b.as_slice());
    let kde_cut = get(KDE_CUT).is_some_and(|b| b.trim_ascii() == b"1");
    [GNOME, URI_LIST].iter().filter_map(|m| get(m)).map(crate::shell::parse_file_list).find(|(paths, _)| !paths.is_empty()).map(|(paths, cut)| (paths, cut || kde_cut))
}

type R<T> = Result<T, Box<dyn std::error::Error>>;

fn intern(conn: &XCBConnection, names: &[&str]) -> R<Vec<Atom>> {
    let cookies = names.iter().map(|n| conn.intern_atom(false, n.as_bytes())).collect::<Result<Vec<_>, _>>()?;
    cookies.into_iter().map(|c| Ok(c.reply()?.atom)).collect()
}

pub fn x11_names() -> &'static [(Atom, &'static str)] {
    static NAMES: OnceLock<Vec<(Atom, &'static str)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let found = XCBConnection::connect(None).map_err(Into::into).and_then(|(conn, _)| intern(&conn, &FILE_TYPES));
        found.map(|atoms| atoms.into_iter().zip(FILE_TYPES).collect()).unwrap_or_default()
    })
}

#[derive(Debug, PartialEq)]
pub(crate) struct Chunks {
    len: usize,
    at: usize,
    done: bool,
}

impl Chunks {
    pub(crate) fn new(len: usize) -> Self {
        Self { len, at: 0, done: false }
    }

    pub(crate) fn next_range(&mut self, chunk: usize) -> Option<Range<usize>> {
        if self.done {
            return None;
        }
        let end = (self.at + chunk.max(1)).min(self.len);
        let r = self.at..end;
        self.at = end;
        self.done = r.is_empty();
        Some(r)
    }
}

pub(crate) fn needs_incr(len: usize, max_request: usize) -> bool {
    len + 64 >= max_request.min(4 * INCR_CHUNK)
}

struct Transfer {
    requestor: Window,
    property: Atom,
    target: Atom,
    k: usize,
    chunks: Chunks,
}

struct Owner {
    conn: XCBConnection,
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    types: Vec<(Atom, usize)>,
    payload: Payload,
    transfers: Vec<Transfer>,
}

impl Owner {
    fn start_incr(&mut self, req: &SelectionRequestEvent, property: Atom, k: usize) -> R<()> {
        self.conn.change_window_attributes(req.requestor, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY))?;
        let len = self.payload[k].1.len().min(u32::MAX as usize) as u32;
        self.conn.change_property(PropMode::REPLACE, req.requestor, property, self.incr, 32, 1, &len.to_ne_bytes())?;
        self.transfers.retain(|t| !(t.requestor == req.requestor && t.property == property));
        self.transfers.push(Transfer { requestor: req.requestor, property, target: req.target, k, chunks: Chunks::new(self.payload[k].1.len()) });
        Ok(())
    }

    fn send_chunk(&mut self, window: Window, property: Atom) -> R<()> {
        let Some(n) = self.transfers.iter().position(|t| t.requestor == window && t.property == property) else { return Ok(()) };
        let chunk = INCR_CHUNK.min(self.conn.maximum_request_bytes().saturating_sub(64)).max(1);
        let t = &mut self.transfers[n];
        let Some(r) = t.chunks.next_range(chunk) else { return Ok(()) };
        let data = &self.payload[t.k].1[r.clone()];
        self.conn.change_property(PropMode::REPLACE, window, property, t.target, 8, data.len() as u32, data)?;
        if r.is_empty() {
            self.transfers.remove(n);
            if !self.transfers.iter().any(|t| t.requestor == window) {
                self.conn.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT))?;
            }
        }
        self.conn.flush()?;
        Ok(())
    }

    fn answer(&mut self, req: &SelectionRequestEvent) -> R<()> {
        let property = if req.property == NONE { req.target } else { req.property };
        let stored = if req.selection != self.clipboard {
            false
        } else if req.target == self.targets {
            let mut list: Vec<Atom> = vec![self.targets];
            list.extend(self.types.iter().map(|(a, _)| *a));
            let bytes: Vec<u8> = list.iter().flat_map(|a| a.to_ne_bytes()).collect();
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, 32, list.len() as u32, &bytes)?;
            true
        } else if let Some(&(_, k)) = self.types.iter().find(|(a, _)| *a == req.target) {
            let data = &self.payload[k].1;
            if needs_incr(data.len(), self.conn.maximum_request_bytes()) {
                self.start_incr(req, property, k)?;
            } else {
                self.conn.change_property(PropMode::REPLACE, req.requestor, property, req.target, 8, data.len() as u32, data)?;
            }
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent { response_type: SELECTION_NOTIFY_EVENT, sequence: 0, time: req.time, requestor: req.requestor, selection: req.selection, target: req.target, property: if stored { property } else { NONE } };
        self.conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    fn serve(mut self) {
        loop {
            match self.conn.wait_for_event() {
                Ok(Event::SelectionRequest(req)) => {
                    if let Err(e) = self.answer(&req) {
                        tracing::debug!("cannot answer a clipboard request: {e}");
                    }
                }
                Ok(Event::PropertyNotify(e)) if e.state == Property::DELETE => {
                    if let Err(err) = self.send_chunk(e.window, e.atom) {
                        tracing::debug!("cannot send the next clipboard chunk: {err}");
                        self.transfers.retain(|t| t.requestor != e.window);
                    }
                }
                Ok(Event::DestroyNotify(e)) => self.transfers.retain(|t| t.requestor != e.window),
                Ok(Event::SelectionClear(c)) if c.selection == self.clipboard => self.types.clear(),
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("the clipboard connection closed: {e}");
                    return;
                }
            }
            if self.types.is_empty() && self.transfers.is_empty() {
                return;
            }
        }
    }
}

pub fn is_text(mime: &str) -> bool {
    mime == PLAIN || mime.starts_with("text/plain;") || X11_TEXT.contains(&mime)
}

pub fn watch_x11_text(selection: &str, changed: impl Fn(bool) + Send + 'static) -> R<()> {
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    let (conn, screen) = XCBConnection::connect(None)?;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let root = conn.setup().roots[screen].root;
    let names: Vec<&str> = [selection, "TARGETS", "FLIT_TARGETS", TEXT, PLAIN].into_iter().chain(X11_TEXT).collect();
    let atoms = intern(&conn, &names)?;
    let (clipboard, targets, property, text) = (atoms[0], atoms[1], atoms[2], atoms[3..].to_vec());
    let window = conn.generate_id()?;
    conn.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1))?;
    let mask = SelectionEventMask::SET_SELECTION_OWNER | SelectionEventMask::SELECTION_WINDOW_DESTROY | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    conn.xfixes_select_selection_input(window, clipboard, mask)?;
    let ask = move |conn: &XCBConnection, time: u32| -> R<()> {
        conn.convert_selection(window, clipboard, targets, property, time)?;
        conn.flush()?;
        Ok(())
    };
    if conn.get_selection_owner(clipboard)?.reply()?.owner == NONE {
        changed(false);
    } else {
        ask(&conn, CURRENT_TIME)?;
    }
    conn.flush()?;
    std::thread::Builder::new().name("flit-clipboard-watch".into()).spawn(move || loop {
        match conn.wait_for_event() {
            Ok(Event::XfixesSelectionNotify(e)) if e.selection == clipboard && e.owner == NONE => changed(false),
            Ok(Event::XfixesSelectionNotify(e)) if e.selection == clipboard => {
                if let Err(e) = ask(&conn, e.selection_timestamp) {
                    tracing::debug!("cannot ask the clipboard for its types: {e}");
                }
            }
            Ok(Event::SelectionNotify(n)) if n.selection == clipboard => {
                let reply = (n.property != NONE).then(|| conn.get_property(true, window, property, AtomEnum::ANY, 0, 4096).ok()?.reply().ok()).flatten();
                let listed: Option<Vec<Atom>> = reply.and_then(|r| r.value32().map(Iterator::collect));
                changed(listed.is_none_or(|list| list.iter().any(|a| text.contains(a))));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("the clipboard watch closed: {e}");
                return;
            }
        }
    })?;
    Ok(())
}

pub fn own_x11(payload: Payload) -> R<()> {
    own_selection("CLIPBOARD", payload)
}

pub fn own_selection(selection: &str, payload: Payload) -> R<()> {
    let (conn, screen) = XCBConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let names: Vec<&str> = payload.iter().map(|(t, _)| t.as_str()).chain(X11_TEXT).chain([selection, "TARGETS", "INCR"]).collect();
    let atoms = intern(&conn, &names)?;
    let (clipboard, targets, incr) = (atoms[names.len() - 3], atoms[names.len() - 2], atoms[names.len() - 1]);
    let text = payload.iter().position(|(t, _)| t == TEXT);
    let mut types: Vec<(Atom, usize)> = (0..payload.len()).map(|k| (atoms[k], k)).collect();
    types.extend(text.into_iter().flat_map(|k| atoms[payload.len()..payload.len() + X11_TEXT.len()].iter().map(move |&a| (a, k))));
    let window = conn.generate_id()?;
    conn.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1))?;
    conn.set_selection_owner(window, clipboard, CURRENT_TIME)?;
    if conn.get_selection_owner(clipboard)?.reply()?.owner != window {
        return Err("another program kept the clipboard".into());
    }
    let owner = Owner { conn, clipboard, targets, incr, types, payload, transfers: Vec::new() };
    std::thread::Builder::new().name("flit-clipboard".into()).spawn(move || owner.serve())?;
    Ok(())
}

#[cfg(test)]
#[path = "../test/clipboard.rs"]
mod tests;
