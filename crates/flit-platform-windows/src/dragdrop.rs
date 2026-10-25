use crate::clipboard::{global, global_bytes, parse_dropfiles};
use crate::context_menu::Pidls;
use crate::err;
use flit_core::input::Mods;
use flit_core::ui::DragEvent;
use flit_core::FsPath;
use flit_platform::{DropReply, PlatformError};
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{GlobalFree, HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
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
        let reply = unsafe { (self.call)(self.node, ev) };
        if started.elapsed() > SLOW_DROP_TARGET {
            tracing::warn!("drop target {kind} took {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        }
        let caption = reply.caption.map(|(message, insert)| (reply.effect, message, insert));
        if *self.shown.borrow() != caption {
            if let Some(data) = &*self.data.borrow() {
                describe(data, caption.as_ref());
            }
            *self.shown.borrow_mut() = caption;
        }
        DROPEFFECT(reply.effect)
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
        let (x, y) = self.client(pt);
        *self.data.borrow_mut() = data.as_ref().cloned();
        *self.shown.borrow_mut() = None;
        unsafe {
            self.active.set(!paths.is_empty());
            *effect = if paths.is_empty() { DROPEFFECT_NONE } else { self.deliver(DragEvent::Enter { paths, x, y, mods: mods(keys), allowed: (*effect).0 }) };
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
        self.finish(DragEvent::Leave);
        if let Some(h) = &self.helper {
            let _ = unsafe { h.DragLeave() };
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows_core::Result<()> {
        let (x, y) = self.client(pt);
        unsafe {
            *effect = self.finish(DragEvent::Drop { x, y, mods: mods(keys), allowed: (*effect).0 });
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
        let target: IDropTarget = Target { hwnd, node, call, helper, active: Cell::new(false), data: RefCell::new(None), shown: RefCell::new(None) }.into();
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

fn button_down() -> bool {
    unsafe {
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        GetAsyncKeyState(vk.0 as i32) < 0
    }
}

pub(crate) fn start_drag(hwnd: HWND, paths: &[FsPath]) -> Result<(), PlatformError> {
    if !button_down() {
        return Ok(());
    }
    let started = Instant::now();
    let data: IDataObject = Pidls::parse(paths)?.ui_object(hwnd)?;
    let ready = started.elapsed();
    let result = unsafe { SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK) };
    tracing::info!("drag of {} items: data object {:.1} ms, drag loop {:.1} ms", paths.len(), ready.as_secs_f64() * 1e3, (started.elapsed() - ready).as_secs_f64() * 1e3);
    result.map(drop).map_err(err)
}

#[cfg(test)]
#[path = "../test/dragdrop.rs"]
mod tests;
