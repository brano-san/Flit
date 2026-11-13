use crate::clipboard::{global, global_bytes, parse_dropfiles};
use crate::context_menu::Pidls;
use crate::err;
use crate::transfer::Content;
use flit_core::input::Mods;
use flit_core::ui::DragEvent;
use flit_core::FsPath;
use flit_platform::{DropReply, PlatformError};
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};
use windows::core::{w, BOOL, HRESULT};
use windows::Win32::Foundation::{GlobalFree, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{IDropTargetHelper, SHDoDragDrop, CFSTR_DROPDESCRIPTION, CLSID_DragDropHelper, DROPDESCRIPTION, DROPIMAGETYPE, DROPIMAGE_INVALID};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
use windows_core::{implement, Ref};

pub(crate) type DragCall = unsafe fn(*const (), DragEvent) -> DropReply;

type Caption = (u32, String, String);

const SLOW_DROP_TARGET: Duration = Duration::from_millis(100);

thread_local! {
    static OLE: Cell<bool> = const { Cell::new(false) };
}

#[implement(IDropTarget)]
struct Target {
    hwnd: HWND,
    node: *const (),
    call: DragCall,
    helper: Option<IDropTargetHelper>,
    active: Cell<bool>,
    data: RefCell<Option<IDataObject>>,
    shown: RefCell<Option<Caption>>,
    effect: Cell<DROPEFFECT>,
    offered: RefCell<Option<FsPath>>,
    source_took_folder: Cell<bool>,
    content: Cell<Option<Content>>,
}

impl Target {
    fn interface(hwnd: HWND, node: *const (), call: DragCall, helper: Option<IDropTargetHelper>) -> IDropTarget {
        let target = Target {
            hwnd,
            node,
            call,
            helper,
            active: Cell::new(false),
            data: RefCell::new(None),
            shown: RefCell::new(None),
            effect: Cell::new(DROPEFFECT_NONE),
            offered: RefCell::new(None),
            source_took_folder: Cell::new(false),
            content: Cell::new(None),
        };
        target.into()
    }
}

const SEVEN_ZIP_GET_TRANSFER_ID: u32 = 3;
const SEVEN_ZIP_GET_TRANSFER_SIZE: u32 = 112;
const SEVEN_ZIP_GET_TRANSFER_FLAGS_AT: usize = 64;
const SEVEN_ZIP_SOURCE_EXTRACTS_ITSELF: u32 = 1 << 1;

fn hglobal_format(cf: u32) -> FORMATETC {
    FORMATETC { cfFormat: cf as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn offer_target_folder(data: &IDataObject, folder: Option<&FsPath>) -> bool {
    let wide = folder.map_or_else(|| vec![0], |f| f.to_wide(false));
    let bytes: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
    let format = hglobal_format(unsafe { RegisterClipboardFormatW(w!("7-Zip::SetTargetFolder")) });
    unsafe {
        let Some(h) = global(&bytes) else { return false };
        let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: ManuallyDrop::new(None) };
        let accepted = data.SetData(&format, &medium, false).is_ok();
        let _ = GlobalFree(Some(h));
        accepted && data.QueryGetData(&format) != S_OK
    }
}

fn seven_zip_extracts_itself(data: &IDataObject) -> bool {
    let format = hglobal_format(unsafe { RegisterClipboardFormatW(w!("7-Zip::GetTransfer")) });
    let Some(bytes) = (unsafe {
        let Ok(mut medium) = data.GetData(&format) else { return true };
        let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 { global_bytes(medium.u.hGlobal) } else { None };
        ReleaseStgMedium(&mut medium);
        bytes
    }) else {
        return true;
    };
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    match (word(0), word(4), word(SEVEN_ZIP_GET_TRANSFER_FLAGS_AT)) {
        (Some(SEVEN_ZIP_GET_TRANSFER_ID), Some(size), Some(flags)) if size >= SEVEN_ZIP_GET_TRANSFER_SIZE => flags & SEVEN_ZIP_SOURCE_EXTRACTS_ITSELF != 0,
        _ => true,
    }
}

pub(crate) fn data_paths(data: &IDataObject) -> Vec<FsPath> {
    let format = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let Ok(mut medium) = data.GetData(&format) else { return Vec::new() };
        let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 { global_bytes(medium.u.hGlobal) } else { None };
        ReleaseStgMedium(&mut medium);
        bytes.map(|b| parse_dropfiles(&b)).unwrap_or_default()
    }
}

fn description_format() -> FORMATETC {
    let cf = unsafe { RegisterClipboardFormatW(CFSTR_DROPDESCRIPTION) };
    FORMATETC { cfFormat: cf as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn wide_field(s: &str) -> [u16; 260] {
    let mut out = [0u16; 260];
    for (d, c) in out.iter_mut().take(259).zip(s.encode_utf16()) {
        *d = c;
    }
    out
}

pub(crate) fn describe(data: &IDataObject, caption: Option<&Caption>) {
    let d = match caption {
        Some((effect, message, insert)) => DROPDESCRIPTION { r#type: DROPIMAGETYPE(*effect as i32), szMessage: wide_field(message), szInsert: wide_field(insert) },
        None => DROPDESCRIPTION { r#type: DROPIMAGE_INVALID, ..Default::default() },
    };
    unsafe {
        let bytes = std::slice::from_raw_parts(&d as *const DROPDESCRIPTION as *const u8, size_of::<DROPDESCRIPTION>());
        let Some(h) = global(bytes) else { return };
        let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: ManuallyDrop::new(None) };
        if data.SetData(&description_format(), &medium, true).is_err() {
            let _ = GlobalFree(Some(h));
        }
    }
}

fn mods(keys: MODIFIERKEYS_FLAGS) -> Mods {
    Mods { ctrl: keys.0 & MK_CONTROL.0 != 0, shift: keys.0 & MK_SHIFT.0 != 0, alt: keys.0 & MK_ALT != 0 }
}

fn right_button(keys: MODIFIERKEYS_FLAGS) -> bool {
    keys.0 & MK_RBUTTON.0 != 0 && keys.0 & MK_LBUTTON.0 == 0
}

fn continue_drag(escape: bool, keys: MODIFIERKEYS_FLAGS, right: bool) -> HRESULT {
    let (held, other) = if right { (MK_RBUTTON, MK_LBUTTON) } else { (MK_LBUTTON, MK_RBUTTON) };
    if escape || keys.0 & other.0 != 0 {
        DRAGDROP_S_CANCEL
    } else if keys.0 & held.0 == 0 {
        DRAGDROP_S_DROP
    } else {
        S_OK
    }
}

#[implement(IDropSource)]
struct RightSource;

impl IDropSource_Impl for RightSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        continue_drag(escape.as_bool(), keys, true)
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

impl Target_Impl {
    fn client(&self, pt: &POINTL) -> (f32, f32) {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        (p.x as f32, p.y as f32)
    }

    fn deliver(&self, ev: DragEvent) -> DROPEFFECT {
        let started = Instant::now();
        let kind = match ev {
            DragEvent::Enter { .. } => "enter",
            DragEvent::Over { .. } => "over",
            DragEvent::Leave => "leave",
            DragEvent::Drop { .. } => "drop",
        };
        let hovering = matches!(ev, DragEvent::Enter { .. } | DragEvent::Over { .. });
        let reply = unsafe { (self.call)(self.node, ev) };
        if started.elapsed() > SLOW_DROP_TARGET {
            tracing::warn!("drop target {kind} took {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        }
        if hovering {
            self.offer_folder(reply.folder.clone());
        }
        self.effect.set(DROPEFFECT(reply.effect));
        let caption = reply.caption.map(|(message, insert)| (reply.effect, message, insert));
        if *self.shown.borrow() != caption {
            if let Some(data) = &*self.data.borrow() {
                describe(data, caption.as_ref());
            }
            *self.shown.borrow_mut() = caption;
        }
        DROPEFFECT(reply.effect)
    }

    fn offer_folder(&self, folder: Option<FsPath>) {
        if *self.offered.borrow() == folder {
            return;
        }
        let Some(data) = self.data.borrow().clone() else { return };
        self.source_took_folder.set(offer_target_folder(&data, folder.as_ref()) && folder.is_some());
        *self.offered.borrow_mut() = folder;
    }

    fn finish(&self, ev: DragEvent) -> DROPEFFECT {
        let effect = if self.active.replace(false) { self.deliver(ev) } else { DROPEFFECT_NONE };
        self.data.borrow_mut().take();
        effect
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows_core::Result<()> {
        let paths = data.as_ref().map(data_paths).unwrap_or_default();
        let content = if paths.is_empty() { data.as_ref().and_then(crate::transfer::content_of) } else { None };
        let (x, y) = self.client(pt);
        *self.data.borrow_mut() = data.as_ref().cloned();
        *self.shown.borrow_mut() = None;
        *self.offered.borrow_mut() = None;
        self.source_took_folder.set(false);
        self.content.set(content);
        unsafe {
            let active = !paths.is_empty() || content.is_some();
            self.active.set(active);
            *effect = if active { self.deliver(DragEvent::Enter { paths, x, y, mods: mods(keys), allowed: (*effect).0, right: right_button(keys) }) } else { DROPEFFECT_NONE };
            if let (Some(h), Some(d)) = (&self.helper, data.as_ref()) {
                let _ = h.DragEnter(self.hwnd, d, &POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows_core::Result<()> {
        let (x, y) = self.client(pt);
        unsafe {
            *effect = if self.active.get() { self.deliver(DragEvent::Over { x, y, mods: mods(keys), allowed: (*effect).0 }) } else { DROPEFFECT_NONE };
            if let Some(h) = &self.helper {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows_core::Result<()> {
        self.offer_folder(None);
        self.finish(DragEvent::Leave);
        if let Some(h) = &self.helper {
            let _ = unsafe { h.DragLeave() };
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows_core::Result<()> {
        let (x, y) = self.client(pt);
        let source_extracts = self.active.get() && self.source_took_folder.get() && data.as_ref().is_some_and(seven_zip_extracts_itself);
        let shown = self.effect.get();
        let (content, folder) = (self.content.take(), self.offered.borrow().clone());
        unsafe {
            *effect = if source_extracts {
                self.finish(DragEvent::Leave);
                shown
            } else {
                self.finish(DragEvent::Drop { x, y, mods: mods(keys), allowed: (*effect).0 })
            };
            if let (Some(kind), Some(folder), Some(d)) = (content, folder, data.as_ref()) {
                if *effect != DROPEFFECT_NONE {
                    crate::transfer::drop_into(d, kind, &folder, *effect == DROPEFFECT_MOVE);
                    *effect = DROPEFFECT_COPY;
                }
            }
            if let (Some(h), Some(d)) = (&self.helper, data.as_ref()) {
                let _ = h.Drop(d, &POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        Ok(())
    }
}

pub(crate) fn register(hwnd: HWND, node: *const (), call: DragCall) {
    let started = Instant::now();
    unsafe {
        if !OLE.replace(true) {
            let _ = OleInitialize(None);
        }
        let helper = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok();
        let target = Target::interface(hwnd, node, call, helper);
        if let Err(e) = RegisterDragDrop(hwnd, &target) {
            tracing::warn!("drop target: {e}");
        }
    }
    tracing::info!("drop target registered in {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
}

pub(crate) fn revoke(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

fn held_button() -> Option<bool> {
    unsafe {
        let (primary, secondary) = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { (VK_RBUTTON, VK_LBUTTON) } else { (VK_LBUTTON, VK_RBUTTON) };
        let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| GetAsyncKeyState(vk.0 as i32) < 0;
        if down(primary) {
            Some(false)
        } else {
            down(secondary).then_some(true)
        }
    }
}

pub(crate) fn start_drag(hwnd: HWND, paths: &[FsPath]) -> Result<(), PlatformError> {
    let Some(right) = held_button() else { return Ok(()) };
    let started = Instant::now();
    let data: IDataObject = Pidls::parse(paths)?.ui_object(hwnd)?;
    let ready = started.elapsed();
    let source: Option<IDropSource> = right.then(|| RightSource.into());
    let result = unsafe { SHDoDragDrop(Some(hwnd), &data, source.as_ref(), DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK) };
    tracing::info!("drag of {} items: data object {:.1} ms, drag loop {:.1} ms", paths.len(), ready.as_secs_f64() * 1e3, (started.elapsed() - ready).as_secs_f64() * 1e3);
    result.map(drop).map_err(err)
}

#[cfg(test)]
#[path = "../test/dragdrop.rs"]
mod tests;
