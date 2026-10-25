use crate::clipboard::{global_bytes, parse_dropfiles};
use crate::context_menu::Pidls;
use crate::err;
use flit_core::input::Mods;
use flit_core::ui::DragEvent;
use flit_core::FsPath;
use flit_platform::PlatformError;
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{IDropTargetHelper, SHDoDragDrop, CLSID_DragDropHelper};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
use windows_core::{implement, Ref};

pub(crate) type DragCall = unsafe fn(*const (), DragEvent) -> u32;

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
        DROPEFFECT(unsafe { (self.call)(self.node, ev) })
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows_core::Result<()> {
        let paths = data.as_ref().map(data_paths).unwrap_or_default();
        let (x, y) = self.client(pt);
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
        if self.active.replace(false) {
            self.deliver(DragEvent::Leave);
        }
        if let Some(h) = &self.helper {
            let _ = unsafe { h.DragLeave() };
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows_core::Result<()> {
        let (x, y) = self.client(pt);
        unsafe {
            *effect = if self.active.replace(false) { self.deliver(DragEvent::Drop { x, y, mods: mods(keys), allowed: (*effect).0 }) } else { DROPEFFECT_NONE };
            if let (Some(h), Some(d)) = (&self.helper, data.as_ref()) {
                let _ = h.Drop(d, &POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        Ok(())
    }
}

pub(crate) fn register(hwnd: HWND, node: *const (), call: DragCall) {
    unsafe {
        if !OLE.replace(true) {
            let _ = OleInitialize(None);
        }
        let helper = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok();
        let target: IDropTarget = Target { hwnd, node, call, helper, active: Cell::new(false) }.into();
        if let Err(e) = RegisterDragDrop(hwnd, &target) {
            tracing::warn!("drop target: {e}");
        }
    }
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
    let data: IDataObject = Pidls::parse(paths)?.ui_object(hwnd)?;
    unsafe { SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK) }.map(drop).map_err(err)
}
