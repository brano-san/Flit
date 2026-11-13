use flit_core::FsPath;
use std::sync::OnceLock;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent, SelectionRequestEvent, WindowClass, SELECTION_NOTIFY_EVENT};
use x11rb::protocol::Event;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

pub const GNOME: &str = "x-special/gnome-copied-files";
pub const KDE_CUT: &str = "application/x-kde-cutselection";
pub const URI_LIST: &str = "text/uri-list";
pub const TEXT: &str = "text/plain;charset=utf-8";
pub const PLAIN: &str = "text/plain";
pub const MARKER: &str = "application/x-flit-files";
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

struct Owner {
    conn: XCBConnection,
    clipboard: Atom,
    targets: Atom,
    types: Vec<(Atom, usize)>,
    payload: Payload,
}

impl Owner {
    fn answer(&self, req: &SelectionRequestEvent) -> R<()> {
        let property = if req.property == NONE { req.target } else { req.property };
        let stored = if req.selection != self.clipboard {
            false
        } else if req.target == self.targets {
            let mut list: Vec<Atom> = vec![self.targets];
            list.extend(self.types.iter().map(|(a, _)| *a));
            let bytes: Vec<u8> = list.iter().flat_map(|a| a.to_ne_bytes()).collect();
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, 32, list.len() as u32, &bytes)?;
            true
        } else if let Some((_, k)) = self.types.iter().find(|(a, _)| *a == req.target).filter(|(_, k)| self.payload[*k].1.len() + 64 < self.conn.maximum_request_bytes()) {
            let data = &self.payload[*k].1;
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, req.target, 8, data.len() as u32, data)?;
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent { response_type: SELECTION_NOTIFY_EVENT, sequence: 0, time: req.time, requestor: req.requestor, selection: req.selection, target: req.target, property: if stored { property } else { NONE } };
        self.conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    fn serve(self) {
        loop {
            match self.conn.wait_for_event() {
                Ok(Event::SelectionRequest(req)) => {
                    if let Err(e) = self.answer(&req) {
                        tracing::debug!("cannot answer a clipboard request: {e}");
                    }
                }
                Ok(Event::SelectionClear(c)) if c.selection == self.clipboard => return,
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("the clipboard connection closed: {e}");
                    return;
                }
            }
        }
    }
}

pub fn own_x11(payload: Payload) -> R<()> {
    own_selection("CLIPBOARD", payload)
}

pub fn own_selection(selection: &str, payload: Payload) -> R<()> {
    let (conn, screen) = XCBConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let names: Vec<&str> = payload.iter().map(|(t, _)| t.as_str()).chain(X11_TEXT).chain([selection, "TARGETS"]).collect();
    let atoms = intern(&conn, &names)?;
    let (clipboard, targets) = (atoms[names.len() - 2], atoms[names.len() - 1]);
    let text = payload.iter().position(|(t, _)| t == TEXT);
    let mut types: Vec<(Atom, usize)> = (0..payload.len()).map(|k| (atoms[k], k)).collect();
    types.extend(text.into_iter().flat_map(|k| atoms[payload.len()..payload.len() + X11_TEXT.len()].iter().map(move |&a| (a, k))));
    let window = conn.generate_id()?;
    conn.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1))?;
    conn.set_selection_owner(window, clipboard, CURRENT_TIME)?;
    if conn.get_selection_owner(clipboard)?.reply()?.owner != window {
        return Err("another program kept the clipboard".into());
    }
    let owner = Owner { conn, clipboard, targets, types, payload };
    std::thread::Builder::new().name("flit-clipboard".into()).spawn(move || owner.serve())?;
    Ok(())
}

#[cfg(test)]
#[path = "../test/clipboard.rs"]
mod tests;
