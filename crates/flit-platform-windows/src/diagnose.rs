use crate::context_menu::Pidls;
use flit_core::FsPath;
use std::path::PathBuf;
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1};
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::ProcessStatus::EnumProcessModules;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Shell::{IContextMenu, CMF_NORMAL};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu};

pub fn attach_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

pub fn gpus() -> Vec<String> {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return Vec::new() };
    (0..)
        .map_while(|k| unsafe { factory.EnumAdapters1(k) }.ok())
        .filter_map(|a| {
            let d = unsafe { a.GetDesc1() }.ok()?;
            let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            let name = String::from_utf16_lossy(&d.Description[..n]);
            let driver = unsafe { a.CheckInterfaceSupport(&IDXGIDevice::IID) }.map_or_else(|_| "unknown".into(), |v| {
                let v = v as u64;
                format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xFFFF, (v >> 16) & 0xFFFF, v & 0xFFFF)
            });
            Some(format!("{name} (vendor 0x{:04X}, device 0x{:04X}, {} MB dedicated), driver {driver}", d.VendorId, d.DeviceId, d.DedicatedVideoMemory >> 20))
        })
        .collect()
}

pub fn load_shell_extensions(folder: &FsPath) {
    crate::com_sta();
    let Ok(pidls) = Pidls::parse(std::slice::from_ref(folder)) else { return };
    let Ok(menu) = pidls.ui_object::<IContextMenu>(HWND::default()) else { return };
    unsafe {
        if let Ok(hmenu) = CreatePopupMenu() {
            let _ = menu.QueryContextMenu(hmenu, 0, 1, 0x7FFF, CMF_NORMAL);
            let _ = DestroyMenu(hmenu);
        }
    }
}

pub fn modules() -> Vec<PathBuf> {
    let mut handles = vec![HMODULE::default(); 1024];
    let mut needed = 0u32;
    let size = (handles.len() * size_of::<HMODULE>()) as u32;
    if unsafe { EnumProcessModules(GetCurrentProcess(), handles.as_mut_ptr(), size, &mut needed) }.is_err() {
        return Vec::new();
    }
    handles.truncate(needed as usize / size_of::<HMODULE>());
    handles
        .into_iter()
        .filter_map(|m| {
            let mut buf = [0u16; 1024];
            let n = unsafe { GetModuleFileNameW(Some(m), &mut buf) } as usize;
            (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
        })
        .collect()
}

pub fn third_party(modules: &[PathBuf]) -> Vec<PathBuf> {
    let windir = std::env::var_os("SystemRoot").map(|w| w.to_string_lossy().to_lowercase()).unwrap_or_else(|| r"c:\windows".into());
    let exe = std::env::current_exe().ok();
    modules.iter().filter(|m| !m.to_string_lossy().to_lowercase().starts_with(&windir) && Some(*m) != exe.as_ref()).cloned().collect()
}

#[cfg(test)]
#[path = "../test/diagnose.rs"]
mod tests;
