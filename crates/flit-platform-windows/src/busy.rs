use flit_core::FsPath;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::RestartManager::{RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO};

const LIST_TRIES: usize = 3;

fn app_name(info: &RM_PROCESS_INFO) -> String {
    let name = &info.strAppName;
    String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())])
}

pub(crate) fn holders(path: &FsPath) -> Vec<String> {
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    if unsafe { RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let file = path.to_wide(true);
    let mut found = Vec::new();
    if unsafe { RmRegisterResources(session, Some(&[PCWSTR(file.as_ptr())]), None, None) } == ERROR_SUCCESS {
        let mut infos = vec![RM_PROCESS_INFO::default(); 4];
        for _ in 0..LIST_TRIES {
            let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
            match unsafe { RmGetList(session, &mut needed, &mut count, Some(infos.as_mut_ptr()), &mut reasons) } {
                ERROR_SUCCESS => {
                    found = infos[..(count as usize).min(infos.len())].iter().map(app_name).collect();
                    break;
                }
                ERROR_MORE_DATA => infos.resize(needed as usize, RM_PROCESS_INFO::default()),
                _ => break,
            }
        }
    }
    unsafe {
        let _ = RmEndSession(session);
    }
    found
}

#[cfg(test)]
#[path = "../test/busy.rs"]
mod tests;
