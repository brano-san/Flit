use flit_core::i18n::{Lang, S};
use flit_core::input::Mods;
use flit_core::ui::{drop_effect, Bitmap};
use flit_core::FsPath;
use std::ffi::c_void;
use std::sync::OnceLock;
use winit::event_loop::DndAction;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
type R<T> = Result<T, Box<dyn std::error::Error>>;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, GrabMode, GrabStatus, KeyButMask, PropMode, Rectangle, SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window, WindowClass,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

const VERSION: u32 = 5;
const GRAB_TRIES: u32 = 50;
const FINISH_WAIT: Duration = Duration::from_secs(5);

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndSelection,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        XdndActionMove,
        XdndActionLink,
        XdndActionAsk,
        XdndActionList,
        XdndActionDescription,
        XdndTypeList,
        TARGETS,
        UriList: b"text/uri-list",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Dropped(Window),
    Refused,
    Canceled,
    Nowhere,
}

const ESCAPE: u32 = 0xff1b;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suggested {
    Any,
    Copy,
    Move,
    Link,
    Ask,
}

pub fn suggested(mask: KeyButMask) -> Suggested {
    let (ctrl, shift, alt) = (mask.contains(KeyButMask::CONTROL), mask.contains(KeyButMask::SHIFT), mask.contains(KeyButMask::MOD1));
    if ctrl && shift {
        Suggested::Link
    } else if ctrl {
        Suggested::Copy
    } else if shift {
        Suggested::Move
    } else if alt {
        Suggested::Ask
    } else {
        Suggested::Any
    }
}

pub fn offered_by_source(action: Suggested) -> Vec<Suggested> {
    match action {
        Suggested::Any | Suggested::Ask => vec![Suggested::Copy, Suggested::Move, Suggested::Link, Suggested::Ask],
        one => vec![one],
    }
}

fn latin1(text: &str) -> Option<Vec<u8>> {
    text.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect()
}

pub fn action_description(actions: &[Suggested], l: Lang) -> Option<Vec<u8>> {
    if !actions.contains(&Suggested::Ask) {
        return None;
    }
    let label = |a: Suggested| match a {
        Suggested::Copy | Suggested::Any => S::UiDropCopyHere,
        Suggested::Move => S::UiDropMoveHere,
        Suggested::Link => S::UiDropLinkHere,
        Suggested::Ask => S::SettingsChoiceConflictAsk,
    };
    let texts: Vec<Vec<u8>> = actions.iter().map(|&a| latin1(label(a).in_lang(l)).or_else(|| latin1(label(a).en())).unwrap_or_default()).collect();
    Some(texts.join(&0))
}

pub fn codes_with(keysyms: &[u32], per_code: u8, first: u8, wanted: u32) -> Vec<u8> {
    keysyms.chunks(usize::from(per_code.max(1))).enumerate().filter(|(_, syms)| syms.contains(&wanted)).filter_map(|(k, _)| u8::try_from(usize::from(first) + k).ok()).collect()
}

pub fn drop_choice(keys: Option<Mods>, proposed: Option<DndAction>, preferred: Option<DndAction>) -> (Mods, bool) {
    let held = keys.unwrap_or_else(|| match proposed.filter(|&p| Some(p) != preferred) {
        Some(DndAction::Copy) => Mods { ctrl: true, ..Mods::default() },
        Some(DndAction::Move) => Mods { shift: true, ..Mods::default() },
        Some(DndAction::Ask) => Mods { alt: true, ..Mods::default() },
        _ => Mods::default(),
    });
    (Mods { alt: false, ..held }, held.alt)
}

pub fn incoming_choice(content: bool, keys: Option<Mods>, proposed: Option<DndAction>, preferred: Option<DndAction>) -> (Mods, bool) {
    if content { (Mods::default(), false) } else { drop_choice(keys, proposed, preferred) }
}

pub fn incoming_actions(content: bool, effect: u32, preferred: Option<DndAction>) -> Vec<DndAction> {
    if content {
        if effect & drop_effect::COPY != 0 { vec![DndAction::Copy] } else { Vec::new() }
    } else {
        offered_actions(effect, preferred)
    }
}
pub fn chose_freely(keys: Option<Mods>, proposed: Option<DndAction>, preferred: Option<DndAction>) -> bool {
    keys.is_some() || proposed.is_none_or(|p| Some(p) == preferred)
}

pub fn wire_action(effect: u32) -> Option<DndAction> {
    if effect & drop_effect::MOVE != 0 {
        Some(DndAction::Move)
    } else if effect & (drop_effect::COPY | drop_effect::LINK) != 0 {
        Some(DndAction::Copy)
    } else {
        None
    }
}

pub fn offered_actions(effect: u32, preferred: Option<DndAction>) -> Vec<DndAction> {
    let Some(first) = preferred.or(wire_action(effect)).filter(|_| effect != drop_effect::NONE) else { return Vec::new() };
    let mut out = vec![first];
    out.extend([DndAction::Copy, DndAction::Move, DndAction::Ask].into_iter().filter(|&a| a != first));
    out
}

pub fn pointer_mods() -> Option<Mods> {
    static POINTER: OnceLock<Option<(XCBConnection, Window)>> = OnceLock::new();
    let (conn, root) = POINTER.get_or_init(|| XCBConnection::connect(None).ok().map(|(conn, screen)| {
        let root = conn.setup().roots[screen].root;
        (conn, root)
    })).as_ref()?;
    let mask = conn.query_pointer(*root).ok()?.reply().ok()?.mask;
    Some(Mods { ctrl: mask.contains(KeyButMask::CONTROL), shift: mask.contains(KeyButMask::SHIFT), alt: mask.contains(KeyButMask::MOD1) })
}

pub fn release_pointer_of(display: *mut c_void) {
    type Ungrab = unsafe extern "C" fn(*mut c_void, u64) -> i32;
    type Flush = unsafe extern "C" fn(*mut c_void) -> i32;
    let lib = unsafe { libc::dlopen(c"libX11.so.6".as_ptr(), libc::RTLD_LAZY | libc::RTLD_NOLOAD) };
    if lib.is_null() || display.is_null() {
        return;
    }
    let (ungrab, flush) = unsafe { (libc::dlsym(lib, c"XUngrabPointer".as_ptr()), libc::dlsym(lib, c"XFlush".as_ptr())) };
    if !ungrab.is_null() && !flush.is_null() {
        unsafe {
            std::mem::transmute::<*mut c_void, Ungrab>(ungrab)(display, 0);
            std::mem::transmute::<*mut c_void, Flush>(flush)(display);
        }
    }
    unsafe { libc::dlclose(lib) };
}

struct Source {
    conn: XCBConnection,
    root: Window,
    window: Window,
    ghost: Window,
    atoms: Atoms,
    data: Vec<u8>,
    depth: u8,
    pixels: Option<(u16, u16, Vec<u8>)>,
    action: std::cell::Cell<Suggested>,
}

const GHOST_BACKGROUND: [u8; 3] = [0xf4, 0xf4, 0xf4];

fn bgrx_screen(setup: &x11rb::protocol::xproto::Setup, screen: &x11rb::protocol::xproto::Screen) -> bool {
    let wide = setup.pixmap_formats.iter().any(|f| f.depth == screen.root_depth && f.bits_per_pixel == 32);
    let rgb = screen.allowed_depths.iter().flat_map(|d| &d.visuals).any(|v| v.visual_id == screen.root_visual && (v.red_mask, v.green_mask, v.blue_mask) == (0xff_0000, 0xff00, 0xff));
    setup.image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST && wide && rgb
}

struct Target {
    window: Window,
    version: u32,
    waiting: bool,
    pending: Option<(i16, i16, Timestamp)>,
    accepted: bool,
}

impl Source {
    fn open(data: Vec<u8>, picture: &Bitmap) -> R<Self> {
        let (conn, screen) = XCBConnection::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let window = conn.generate_id()?;
        conn.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1))?;
        conn.set_selection_owner(window, atoms.XdndSelection, CURRENT_TIME)?;
        let ghost = conn.generate_id()?;
        let screen = &conn.setup().roots[screen];
        let pixels = bgrx_screen(conn.setup(), screen).then(|| (picture.width as u16, picture.height as u16, over_background(picture, GHOST_BACKGROUND)));
        let (w, h, border) = pixels.as_ref().map_or((40, 40, 1), |p| (p.0, p.1, 0));
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0xf4f4f4).border_pixel(0x808080).event_mask(EventMask::EXPOSURE);
        conn.create_window(screen.root_depth, ghost, root, -100, -100, w, h, border, WindowClass::INPUT_OUTPUT, screen.root_visual, &aux)?;
        let runs = opaque_runs(picture);
        if pixels.is_some() && !runs.is_empty() {
            use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
            conn.shape_rectangles(SO::SET, SK::BOUNDING, x11rb::protocol::xproto::ClipOrdering::UNSORTED, ghost, 0, 0, &runs)?;
        }
        let pointer = conn.query_pointer(root)?.reply()?;
        conn.configure_window(ghost, &x11rb::protocol::xproto::ConfigureWindowAux::new().x(i32::from(pointer.root_x) + 16).y(i32::from(pointer.root_y) + 16))?;
        conn.map_window(ghost)?;
        conn.flush()?;
        let depth = screen.root_depth;
        Ok(Self { conn, root, window, ghost, atoms, data, depth, pixels, action: std::cell::Cell::new(Suggested::Any) })
    }

    fn paint(&self) -> R<()> {
        use x11rb::protocol::xproto::{CreateGCAux, ImageFormat};
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, self.ghost, &CreateGCAux::new().foreground(0xf0bc48))?;
        match &self.pixels {
            Some((w, h, bytes)) => self.conn.put_image(ImageFormat::Z_PIXMAP, self.ghost, gc, *w, *h, 0, 0, 0, self.depth, bytes)?,
            None => self.conn.poly_fill_rectangle(self.ghost, gc, &[Rectangle { x: 4, y: 6, width: 15, height: 9 }, Rectangle { x: 4, y: 12, width: 32, height: 22 }])?,
        };
        self.conn.free_gc(gc)?;
        self.conn.flush()?;
        Ok(())
    }

    fn grab(&self) -> R<bool> {
        let mask = EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::BUTTON_MOTION;
        for _ in 0..GRAB_TRIES {
            if self.conn.grab_pointer(false, self.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?.reply()?.status == GrabStatus::SUCCESS {
                if self.conn.grab_keyboard(false, self.root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?.status != GrabStatus::SUCCESS {
                    tracing::debug!("the keyboard stays with another client: Esc will not cancel this drag");
                }
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(false)
    }

    fn escape_codes(&self) -> Vec<u8> {
        let setup = self.conn.setup();
        let count = setup.max_keycode.saturating_sub(setup.min_keycode).saturating_add(1);
        let Ok(Ok(map)) = self.conn.get_keyboard_mapping(setup.min_keycode, count).map(|c| c.reply()) else { return Vec::new() };
        codes_with(&map.keysyms, map.keysyms_per_keycode, setup.min_keycode, ESCAPE)
    }

    fn atom_of(&self, action: Suggested) -> Atom {
        match action {
            Suggested::Copy | Suggested::Any => self.atoms.XdndActionCopy,
            Suggested::Move => self.atoms.XdndActionMove,
            Suggested::Link => self.atoms.XdndActionLink,
            Suggested::Ask => self.atoms.XdndActionAsk,
        }
    }

    fn offer(&self, action: Suggested) -> R<()> {
        let offered = offered_by_source(action);
        let list: Vec<Atom> = offered.iter().map(|&a| self.atom_of(a)).collect();
        self.conn.change_property32(PropMode::REPLACE, self.window, self.atoms.XdndActionList, AtomEnum::ATOM, &list)?;
        match action_description(&offered, flit_core::i18n::lang()) {
            Some(text) => self.conn.change_property8(PropMode::REPLACE, self.window, self.atoms.XdndActionDescription, AtomEnum::STRING, &text)?,
            None => self.conn.delete_property(self.window, self.atoms.XdndActionDescription)?,
        };
        Ok(())
    }

    fn held(&self) -> Suggested {
        self.conn.query_pointer(self.root).ok().and_then(|c| c.reply().ok()).map_or(Suggested::Any, |p| suggested(p.mask))
    }

    fn aware(&self, w: Window) -> Option<u32> {
        let reply = self.conn.get_property(false, w, self.atoms.XdndAware, AtomEnum::ATOM, 0, 1).ok()?.reply().ok()?;
        let first = reply.value32()?.next();
        first
    }

    fn target_at(&self) -> Option<(Window, u32)> {
        let mut w = self.root;
        loop {
            if w != self.root {
                if let Some(v) = self.aware(w) {
                    return Some((w, v.min(VERSION)));
                }
            }
            let child = self.conn.query_pointer(w).ok()?.reply().ok()?.child;
            if child == NONE {
                return None;
            }
            w = child;
        }
    }

    fn send(&self, to: Window, kind: Atom, data: [u32; 5]) -> R<()> {
        self.conn.send_event(false, to, EventMask::NO_EVENT, ClientMessageEvent::new(32, to, kind, data))?;
        self.conn.flush()?;
        Ok(())
    }

    fn enter(&self, t: &Target) -> R<()> {
        self.send(t.window, self.atoms.XdndEnter, [self.window, t.version << 24, self.atoms.UriList, 0, 0])
    }

    fn leave(&self, t: &Target) -> R<()> {
        self.send(t.window, self.atoms.XdndLeave, [self.window, 0, 0, 0, 0])
    }

    fn position(&self, t: &mut Target, (x, y, time): (i16, i16, Timestamp)) -> R<()> {
        if t.waiting {
            t.pending = Some((x, y, time));
            return Ok(());
        }
        t.waiting = true;
        t.pending = None;
        let at = ((x as u16 as u32) << 16) | y as u16 as u32;
        self.send(t.window, self.atoms.XdndPosition, [self.window, 0, at, time, self.atom_of(self.action.get())])
    }

    fn choose(&self, action: Suggested) -> R<bool> {
        if self.action.replace(action) == action {
            return Ok(false);
        }
        self.offer(action)?;
        Ok(true)
    }

    fn answer(&self, req: &SelectionRequestEvent) -> R<()> {
        let property = if req.property == NONE { req.target } else { req.property };
        let stored = if req.selection != self.atoms.XdndSelection {
            false
        } else if req.target == self.atoms.UriList {
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, self.atoms.UriList, 8, self.data.len() as u32, &self.data)?;
            true
        } else if req.target == self.atoms.TARGETS {
            let targets = [self.atoms.TARGETS, self.atoms.UriList];
            let bytes: Vec<u8> = targets.iter().flat_map(|a| a.to_ne_bytes()).collect();
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, 32, targets.len() as u32, &bytes)?;
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property: if stored { property } else { NONE },
        };
        self.conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    fn run(&self) -> R<Outcome> {
        let mut target: Option<Target> = None;
        let escape = self.escape_codes();
        let mut last = None;
        self.action.set(self.held());
        self.offer(self.action.get())?;
        loop {
            let event = self.conn.wait_for_event()?;
            match event {
                Event::Expose(e) if e.window == self.ghost => self.paint()?,
                Event::KeyPress(k) if escape.contains(&k.detail) => {
                    self.conn.unmap_window(self.ghost)?;
                    if let Some(t) = target {
                        self.leave(&t)?;
                    }
                    return Ok(Outcome::Canceled);
                }
                Event::KeyPress(_) | Event::KeyRelease(_) => {
                    if self.choose(self.held())? {
                        if let (Some(t), Some(at)) = (target.as_mut(), last) {
                            self.position(t, at)?;
                        }
                    }
                }
                Event::MotionNotify(m) => {
                    last = Some((m.root_x, m.root_y, m.time));
                    self.choose(suggested(m.state))?;
                    self.conn.configure_window(self.ghost, &x11rb::protocol::xproto::ConfigureWindowAux::new().x(i32::from(m.root_x) + 16).y(i32::from(m.root_y) + 16))?;
                    self.conn.flush()?;
                    let found = self.target_at();
                    if target.as_ref().map(|t| t.window) != found.map(|f| f.0) {
                        if let Some(old) = target.take() {
                            self.leave(&old)?;
                        }
                        if let Some((window, version)) = found {
                            let t = Target { window, version, waiting: false, pending: None, accepted: false };
                            self.enter(&t)?;
                            target = Some(t);
                        }
                    }
                    if let Some(t) = target.as_mut() {
                        self.position(t, (m.root_x, m.root_y, m.time))?;
                    }
                }
                Event::ClientMessage(c) if c.type_ == self.atoms.XdndStatus => {
                    let d = c.data.as_data32();
                    if let Some(t) = target.as_mut().filter(|t| t.window == d[0]) {
                        t.waiting = false;
                        t.accepted = d[1] & 1 != 0;
                        if let Some(p) = t.pending.take() {
                            self.position(t, p)?;
                        }
                    }
                }
                Event::SelectionRequest(req) => self.answer(&req)?,
                Event::ButtonRelease(b) => {
                    self.conn.unmap_window(self.ghost)?;
                    let Some(t) = target else { return Ok(Outcome::Nowhere) };
                    if !t.accepted {
                        self.leave(&t)?;
                        return Ok(Outcome::Refused);
                    }
                    self.conn.ungrab_pointer(CURRENT_TIME)?;
                    self.conn.ungrab_keyboard(CURRENT_TIME)?;
                    self.send(t.window, self.atoms.XdndDrop, [self.window, 0, b.time, 0, 0])?;
                    return self.finish(t.window);
                }
                _ => {}
            }
        }
    }

    fn finish(&self, target: Window) -> R<Outcome> {
        let until = Instant::now() + FINISH_WAIT;
        while Instant::now() < until {
            match self.conn.poll_for_event()? {
                Some(Event::SelectionRequest(req)) => self.answer(&req)?,
                Some(Event::ClientMessage(c)) if c.type_ == self.atoms.XdndFinished => return Ok(Outcome::Dropped(target)),
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        Ok(Outcome::Dropped(target))
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.destroy_window(self.ghost);
        let _ = self.conn.flush();
    }
}

const DRAG_ICON: f32 = 48.0;
const DRAG_MARGIN: f32 = 12.0;
const STACK_STEP: f32 = 6.0;
const STACK_OPACITY: f32 = 0.55;
const BADGE_RADIUS: f32 = 11.0;
const BADGE_TEXT: f32 = 13.0;
const BADGE_COLOR: [u8; 4] = [0x35, 0x84, 0xe4, 255];
const FOLDER_COLOR: [u8; 4] = [240, 188, 72, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const MOST_COUNTED: usize = 999;
const PICTURE_WAIT: Duration = Duration::from_millis(40);

pub fn badge_text(count: usize) -> String {
    if count > MOST_COUNTED {
        format!("{MOST_COUNTED}+")
    } else {
        count.to_string()
    }
}

fn blend(canvas: &mut Bitmap, x: i32, y: i32, src: [u8; 4]) {
    if x < 0 || y < 0 || x >= canvas.width as i32 || y >= canvas.height as i32 || src[3] == 0 {
        return;
    }
    let at = (y as usize * canvas.width as usize + x as usize) * 4;
    let keep = 255 - u32::from(src[3]);
    for (k, s) in src.iter().enumerate() {
        canvas.rgba[at + k] = (u32::from(*s) + (u32::from(canvas.rgba[at + k]) * keep + 127) / 255).min(255) as u8;
    }
}

fn tint(color: [u8; 4], coverage: f32) -> [u8; 4] {
    let a = coverage.clamp(0.0, 1.0) * f32::from(color[3]) / 255.0;
    [color[0], color[1], color[2], 255].map(|c| (f32::from(c) * a).round() as u8)
}

fn draw_icon(canvas: &mut Bitmap, icon: &Bitmap, (x0, y0): (i32, i32), side: u32, opacity: f32) {
    let (dx, dy) = (x0 + (side as i32 - icon.width as i32) / 2, y0 + (side as i32 - icon.height as i32) / 2);
    for (k, p) in icon.rgba.chunks_exact(4).enumerate() {
        let (x, y) = ((k % icon.width as usize) as i32, (k / icon.width as usize) as i32);
        blend(canvas, dx + x, dy + y, [p[0], p[1], p[2], p[3]].map(|c| (f32::from(c) * opacity).round() as u8));
    }
}

fn draw_folder(canvas: &mut Bitmap, (x0, y0): (i32, i32), side: u32, opacity: f32) {
    let s = side as f32 / 40.0;
    let color = tint(FOLDER_COLOR, opacity);
    for y in (5.0 * s) as i32..(35.0 * s) as i32 {
        for x in (4.0 * s) as i32..(36.0 * s) as i32 {
            if y as f32 >= 11.0 * s || (x as f32) < 19.0 * s {
                blend(canvas, x0 + x, y0 + y, color);
            }
        }
    }
}

fn fill_pill(canvas: &mut Bitmap, (cx, cy): (f32, f32), width: f32, radius: f32, color: [u8; 4]) {
    let half = (width / 2.0 - radius).max(0.0);
    let (x0, x1) = ((cx - width / 2.0).floor() as i32, (cx + width / 2.0).ceil() as i32);
    let (y0, y1) = ((cy - radius).floor() as i32, (cy + radius).ceil() as i32);
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = ((x as f32 + 0.5 - cx).abs() - half).max(0.0);
            let dy = y as f32 + 0.5 - cy;
            blend(canvas, x, y, tint(color, radius + 0.5 - (dx * dx + dy * dy).sqrt()));
        }
    }
}

type Label = (swash::FontRef<'static>, Vec<(u16, f32)>, f32);

fn label_glyphs(text: &str, px: f32) -> Option<Label> {
    let font = swash::FontRef::from_index(crate::fonts::SANS, 0)?;
    let (charmap, metrics) = (font.charmap(), font.glyph_metrics(&[]).scale(px));
    let mut pen = 0.0;
    let mut glyphs = Vec::new();
    for id in text.chars().map(|c| charmap.map(c)) {
        glyphs.push((id, pen));
        pen += metrics.advance_width(id);
    }
    Some((font, glyphs, pen))
}

fn draw_label(canvas: &mut Bitmap, (font, glyphs, width): Label, (cx, cy): (f32, f32), px: f32, color: [u8; 4]) {
    use swash::scale::{Render, ScaleContext, Source};
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font).size(px).hint(true).build();
    let baseline = (cy + font.metrics(&[]).scale(px).cap_height / 2.0).round() as i32;
    let left = cx - width / 2.0;
    for (id, x) in glyphs {
        let Some(image) = Render::new(&[Source::Outline]).format(swash::zeno::Format::Alpha).render(&mut scaler, id) else { continue };
        let p = image.placement;
        let (ox, oy) = ((left + x).round() as i32 + p.left, baseline - p.top);
        for (k, &a) in image.data.iter().enumerate().take((p.width * p.height) as usize) {
            blend(canvas, ox + (k as u32 % p.width) as i32, oy + (k as u32 / p.width) as i32, tint(color, f32::from(a) / 255.0));
        }
    }
}

pub fn compose_drag(icon: Option<&Bitmap>, count: usize, scale: f32) -> Bitmap {
    let scale = scale.clamp(0.5, 4.0);
    let side = (DRAG_ICON * scale).round() as u32;
    let margin = (DRAG_MARGIN * scale).round() as u32;
    let total = side + margin;
    let mut canvas = Bitmap { width: total, height: total, rgba: vec![0; (total * total * 4) as usize] };
    let layer = |canvas: &mut Bitmap, at: (i32, i32), opacity: f32| match icon.filter(|i| i.width > 0 && i.rgba.len() == (i.width * i.height * 4) as usize) {
        Some(i) => draw_icon(canvas, i, at, side, opacity),
        None => draw_folder(canvas, at, side, opacity),
    };
    if count > 1 {
        let step = (STACK_STEP * scale).round() as i32;
        layer(&mut canvas, (step, margin as i32 - step), STACK_OPACITY);
    }
    layer(&mut canvas, (0, margin as i32), 1.0);
    if count > 1 {
        let (radius, px) = (BADGE_RADIUS * scale, BADGE_TEXT * scale);
        let label = label_glyphs(&badge_text(count), px);
        let width = label.as_ref().map_or(0.0, |l| l.2 + radius).max(radius * 2.0);
        let center = (total as f32 - width / 2.0, radius);
        fill_pill(&mut canvas, center, width, radius, BADGE_COLOR);
        if let Some(label) = label {
            draw_label(&mut canvas, label, center, px, WHITE);
        }
    }
    canvas
}

fn first_icon(paths: &[FsPath], scale: f32) -> Option<Bitmap> {
    use flit_core::ui::{IconKind, IconRequest};
    let first = paths.first()?;
    let dir = flit_platform_linux::host(first).is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.is_dir()));
    let size = (DRAG_ICON * scale.clamp(0.5, 4.0)).round() as u32;
    let req = IconRequest { key: 0, kind: if dir { IconKind::Folder } else { IconKind::File }, path: first.clone(), size, gen: 0, row: 0 };
    crate::icons::icon(&crate::shell::icon_names(&req), size)
}

pub fn drag_picture(paths: &[FsPath], scale: f32) -> Bitmap {
    compose_drag(first_icon(paths, scale).as_ref(), paths.len(), scale)
}

pub fn drag_picture_soon(paths: Vec<FsPath>, scale: f32) -> Bitmap {
    let count = paths.len();
    let (tx, rx) = crossbeam_channel::bounded(1);
    let spawned = std::thread::Builder::new().name("flit-drag-picture".into()).spawn(move || {
        let _ = tx.send(drag_picture(&paths, scale));
    });
    spawned.ok().and_then(|_| rx.recv_timeout(PICTURE_WAIT).ok()).unwrap_or_else(|| compose_drag(None, count, scale))
}

pub fn over_background(image: &Bitmap, background: [u8; 3]) -> Vec<u8> {
    image.rgba.chunks_exact(4).flat_map(|p| {
        let keep = 255 - u32::from(p[3]);
        let mix = |k: usize| (u32::from(p[k]) + (u32::from(background[k]) * keep + 127) / 255).min(255) as u8;
        [mix(2), mix(1), mix(0), 0]
    }).collect()
}

pub fn opaque_runs(image: &Bitmap) -> Vec<Rectangle> {
    let mut out = Vec::new();
    for y in 0..image.height {
        let row = &image.rgba[(y * image.width * 4) as usize..((y + 1) * image.width * 4) as usize];
        let mut start = None;
        for x in 0..=image.width {
            let solid = x < image.width && row[(x * 4 + 3) as usize] >= SOLID_ALPHA;
            match (solid, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    out.push(Rectangle { x: s as i16, y: y as i16, width: (x - s) as u16, height: 1 });
                    start = None;
                }
                _ => {}
            }
        }
    }
    out
}

const SOLID_ALPHA: u8 = 0x60;

pub fn drag(paths: Vec<FsPath>, scale: f32) -> Result<Outcome, String> {
    let picture = drag_picture(&paths, scale);
    let uri_list = crate::shell::uri_list(&paths);
    let source = Source::open(uri_list.into_bytes(), &picture).map_err(|e| e.to_string())?;
    if !source.grab().map_err(|e| e.to_string())? {
        return Err("the pointer is grabbed by another client".into());
    }
    source.run().map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "../test/xdnd.rs"]
mod tests;
