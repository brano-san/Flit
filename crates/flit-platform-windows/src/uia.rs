#![allow(non_upper_case_globals)]

use flit_core::a11y::{Role, Tree, ROOT_ID};
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use windows::core::BSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, VARIANT_FALSE, VARIANT_TRUE, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_I4};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows_core::{implement, Error, IUnknown, IUnknownImpl, Interface, Result, HRESULT};

pub(crate) const WM_A11Y_INVOKE: u32 = WM_APP + 4;

pub(crate) struct Hub {
    hwnd: isize,
    tree: Mutex<Tree>,
}

pub(crate) struct Access {
    pub hub: Arc<Hub>,
    pub root: IRawElementProviderSimple,
}

impl Access {
    pub fn new(hwnd: HWND, tree: Tree) -> Self {
        let hub = Arc::new(Hub { hwnd: hwnd.0 as isize, tree: Mutex::new(tree) });
        let root = Element { hub: hub.clone(), id: ROOT_ID }.into();
        Self { hub, root }
    }

    pub fn update(&self, tree: Tree) {
        let mut cur = self.hub.tree.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::mem::replace(&mut *cur, tree);
        let focused = cur.focus.map(|k| cur.nodes[k].id);
        let picked = cur.nodes.iter().find(|n| n.selected && matches!(n.role, Role::ListItem | Role::DataItem | Role::TreeItem | Role::Tab) && !old.find(n.id).is_some_and(|k| old.nodes[k].selected)).map(|n| n.id);
        drop(cur);
        if focused.is_some() && focused != old.focus.map(|k| old.nodes[k].id) {
            self.raise(focused, UIA_AutomationFocusChangedEventId);
        }
        self.raise(picked, UIA_SelectionItem_ElementSelectedEventId);
    }

    fn raise(&self, id: Option<u64>, event: UIA_EVENT_ID) {
        let Some(id) = id else { return };
        let el: IRawElementProviderSimple = Element { hub: self.hub.clone(), id }.into();
        unsafe {
            let _ = UiaRaiseAutomationEvent(&el, event);
        }
    }
}

#[implement(IRawElementProviderSimple, IRawElementProviderFragment, IRawElementProviderFragmentRoot, IInvokeProvider, ISelectionItemProvider)]
struct Element {
    hub: Arc<Hub>,
    id: u64,
}

fn gone() -> Error {
    Error::from_hresult(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32))
}

fn control_type(role: Role) -> UIA_CONTROLTYPE_ID {
    match role {
        Role::Window | Role::Dialog => UIA_WindowControlTypeId,
        Role::TabList => UIA_TabControlTypeId,
        Role::Tab => UIA_TabItemControlTypeId,
        Role::ToolBar => UIA_ToolBarControlTypeId,
        Role::Button => UIA_ButtonControlTypeId,
        Role::Group => UIA_GroupControlTypeId,
        Role::Link => UIA_HyperlinkControlTypeId,
        Role::Edit => UIA_EditControlTypeId,
        Role::Tree => UIA_TreeControlTypeId,
        Role::TreeItem => UIA_TreeItemControlTypeId,
        Role::List => UIA_ListControlTypeId,
        Role::ListItem => UIA_ListItemControlTypeId,
        Role::DataGrid => UIA_DataGridControlTypeId,
        Role::DataItem => UIA_DataItemControlTypeId,
        Role::Header => UIA_HeaderControlTypeId,
        Role::HeaderItem => UIA_HeaderItemControlTypeId,
        Role::Pane => UIA_PaneControlTypeId,
        Role::Text => UIA_TextControlTypeId,
    }
}

fn variant(vt: VARENUM, set: impl FnOnce(&mut VARIANT_0_0_0)) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = vt;
        set(&mut inner.Anonymous);
    }
    v
}

fn text(s: &str) -> VARIANT {
    variant(VT_BSTR, |a| a.bstrVal = ManuallyDrop::new(BSTR::from(s)))
}

fn flag(b: bool) -> VARIANT {
    variant(VT_BOOL, |a| a.boolVal = if b { VARIANT_TRUE } else { VARIANT_FALSE })
}

fn int(i: i32) -> VARIANT {
    variant(VT_I4, |a| a.lVal = i)
}

impl Element {
    fn hwnd(&self) -> HWND {
        HWND(self.hub.hwnd as _)
    }

    fn with<T>(&self, f: impl FnOnce(&Tree, usize) -> T) -> Result<T> {
        let tree = self.hub.tree.lock().unwrap_or_else(|e| e.into_inner());
        let k = tree.find(self.id).ok_or_else(gone)?;
        Ok(f(&tree, k))
    }

    fn other(&self, id: Option<u64>) -> Result<IRawElementProviderFragment> {
        id.map(|id| Element { hub: self.hub.clone(), id }.into()).ok_or_else(Error::empty)
    }

    fn invokable(role: Role) -> bool {
        matches!(role, Role::Button | Role::Tab | Role::TreeItem | Role::ListItem | Role::DataItem | Role::HeaderItem | Role::Link)
    }
}

impl IRawElementProviderSimple_Impl for Element_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        let role = self.with(|t, k| t.nodes[k].role)?;
        let me = self.to_object();
        match pattern {
            UIA_InvokePatternId if Element::invokable(role) => me.to_interface::<IInvokeProvider>().cast(),
            UIA_SelectionItemPatternId if matches!(role, Role::Tab | Role::TreeItem | Role::ListItem | Role::DataItem) => me.to_interface::<ISelectionItemProvider>().cast(),
            _ => Err(Error::empty()),
        }
    }

    fn GetPropertyValue(&self, prop: UIA_PROPERTY_ID) -> Result<VARIANT> {
        self.with(|t, k| {
            let n = &t.nodes[k];
            match prop {
                UIA_NamePropertyId => text(&n.name),
                UIA_ControlTypePropertyId => int(control_type(n.role).0),
                UIA_HelpTextPropertyId if !n.help.is_empty() => text(&n.help),
                UIA_HelpTextPropertyId => text(&n.value),
                UIA_ValueValuePropertyId => text(&n.value),
                UIA_IsEnabledPropertyId => flag(n.enabled),
                UIA_HasKeyboardFocusPropertyId => flag(t.focus == Some(k)),
                UIA_IsKeyboardFocusablePropertyId => flag(n.role != Role::Text),
                UIA_SelectionItemIsSelectedPropertyId => flag(n.selected),
                UIA_AutomationIdPropertyId if k != 0 => text(&format!("{:x}", n.id)),
                UIA_FrameworkIdPropertyId => text("Flit"),
                _ => VARIANT::default(),
            }
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == ROOT_ID {
            unsafe { UiaHostProviderFromHwnd(self.hwnd()) }
        } else {
            Err(Error::empty())
        }
    }
}

impl IRawElementProviderFragment_Impl for Element_Impl {
    fn Navigate(&self, dir: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let to = self.with(|t, k| {
            let n = &t.nodes[k];
            let pick = match dir {
                NavigateDirection_Parent if k != 0 => Some(n.parent),
                NavigateDirection_NextSibling => t.sibling(k, 1),
                NavigateDirection_PreviousSibling => t.sibling(k, -1),
                NavigateDirection_FirstChild => n.children.first().copied(),
                NavigateDirection_LastChild => n.children.last().copied(),
                _ => None,
            };
            pick.map(|c| t.nodes[c].id)
        })?;
        self.other(to)
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        if self.id == ROOT_ID {
            return Ok(std::ptr::null_mut());
        }
        let parts = [UiaAppendRuntimeId as i32, (self.id >> 32) as i32, self.id as i32];
        unsafe {
            let sa = SafeArrayCreateVector(VT_I4, 0, parts.len() as u32);
            for (i, v) in parts.iter().enumerate() {
                SafeArrayPutElement(sa, &(i as i32), v as *const i32 as *const _)?;
            }
            Ok(sa)
        }
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        if self.id == ROOT_ID {
            return Ok(UiaRect::default());
        }
        let r = self.with(|t, k| t.nodes[k].bounds)?;
        let mut p = POINT { x: r.x as i32, y: r.y as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd(), &mut p);
        }
        Ok(UiaRect { left: p.x as f64, top: p.y as f64, width: r.w as f64, height: r.h as f64 })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Element { hub: self.hub.clone(), id: ROOT_ID }.into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Element_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let mut p = POINT { x: x as i32, y: y as i32 };
        unsafe {
            let _ = ScreenToClient(self.hwnd(), &mut p);
        }
        let tree = self.hub.tree.lock().unwrap_or_else(|e| e.into_inner());
        let id = tree.nodes[tree.at(p.x as f32, p.y as f32)].id;
        drop(tree);
        self.other(Some(id))
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let tree = self.hub.tree.lock().unwrap_or_else(|e| e.into_inner());
        let id = tree.focus.filter(|&k| k != 0).map(|k| tree.nodes[k].id);
        drop(tree);
        self.other(id)
    }
}

impl IInvokeProvider_Impl for Element_Impl {
    fn Invoke(&self) -> Result<()> {
        self.with(|_, _| ())?;
        unsafe { PostMessageW(Some(self.hwnd()), WM_A11Y_INVOKE, WPARAM(self.id as usize), LPARAM(0)) }
    }
}

impl ISelectionItemProvider_Impl for Element_Impl {
    fn Select(&self) -> Result<()> {
        if self.with(|t, k| t.nodes[k].role)? == Role::Tab {
            return IInvokeProvider_Impl::Invoke(self);
        }
        Err(Error::from_hresult(HRESULT(UIA_E_INVALIDOPERATION as i32)))
    }

    fn AddToSelection(&self) -> Result<()> {
        Err(Error::from_hresult(HRESULT(UIA_E_INVALIDOPERATION as i32)))
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        Err(Error::from_hresult(HRESULT(UIA_E_INVALIDOPERATION as i32)))
    }

    fn IsSelected(&self) -> Result<windows_core::BOOL> {
        Ok(self.with(|t, k| t.nodes[k].selected)?.into())
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        let parent = self.with(|t, k| t.nodes[t.nodes[k].parent].id)?;
        Ok(Element { hub: self.hub.clone(), id: parent }.into())
    }
}

#[cfg(test)]
#[path = "../test/uia.rs"]
mod tests;
