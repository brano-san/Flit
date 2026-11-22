use crate::err;
use crate::shell::owner_window;
use flit_core::path::PathKind;
use flit_core::{FsError, FsPath};
use flit_platform::PlatformError;
use std::collections::HashSet;
use std::sync::Mutex;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_CONNECTION_UNAVAIL, ERROR_INVALID_PASSWORD, ERROR_LOGON_FAILURE, ERROR_NOT_AUTHENTICATED, HANDLE, NO_ERROR, WIN32_ERROR};
use windows::Win32::NetworkManagement::WNet::{
    WNetAddConnection3W, WNetCloseEnum, WNetConnectionDialog1W, WNetDisconnectDialog, WNetDisconnectDialog1W, WNetEnumResourceW, WNetGetConnectionW, WNetOpenEnumW, CONNECTDLGSTRUCTW, CONNECT_INTERACTIVE, CONNDLG_USE_MRU, DISCDLGSTRUCTW, DISC_UPDATE_PROFILE, NETRESOURCEW, RESOURCETYPE_DISK, RESOURCE_REMEMBERED,
    WNET_OPEN_ENUM_USAGE,
};
use windows::Win32::NetworkManagement::WindowsFirewall::{NET_FW_PROFILE_TYPE2, NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;

const DRIVE_REMOTE: u32 = 4;
const CANCELLED_BY_USER: u32 = 0xFFFF_FFFF;
pub(crate) fn discovery_warning(enabled: impl FnOnce() -> Option<bool>) -> Option<flit_core::provider::NetworkWarning> {
    (enabled() == Some(false)).then_some(flit_core::provider::NetworkWarning::WindowsDisabled)
}

pub(crate) fn discovery_enabled() -> Option<bool> {
    use windows::Win32::NetworkManagement::WindowsFirewall::{INetFwPolicy2, NetFwPolicy2};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    crate::com_sta();
    let policy: INetFwPolicy2 = unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let active = unsafe { policy.CurrentProfileTypes() }.ok()?;
    discovery_state(
        active,
        |profile| unsafe { policy.get_FirewallEnabled(profile) }.ok().map(|enabled| enabled.0 != 0),
        || unsafe { policy.get_IsRuleGroupCurrentlyEnabled(&windows::core::BSTR::from("@FirewallAPI.dll,-32752")) }.ok().map(|enabled| enabled.0 != 0),
    )
}

fn discovery_state(active: i32, mut firewall_enabled: impl FnMut(NET_FW_PROFILE_TYPE2) -> Option<bool>, group_enabled: impl FnOnce() -> Option<bool>) -> Option<bool> {
    let mut profiles = [NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC].into_iter().filter(|p| active & p.0 != 0).peekable();
    profiles.peek()?;
    for profile in profiles {
        if !firewall_enabled(profile)? {
            return Some(true);
        }
    }
    group_enabled()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn letter(drive: &FsPath) -> Option<String> {
    (drive.kind() == PathKind::Drive).then(|| format!("{}:", drive.as_bytes()[0] as char))
}

fn unescape(part: &str) -> String {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn server_path(uri: &str) -> Result<FsPath, String> {
    let uri = uri.trim();
    if let Some(unc) = FsPath::parse(uri).filter(|p| p.kind() == PathKind::Unc) {
        return Ok(unc);
    }
    let (scheme, rest) = uri.split_once("://").ok_or_else(|| uri.to_owned())?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() => (h, Some(p)),
        _ => (host_port, None),
    };
    let parts: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(unescape).collect();
    let server = match (scheme.as_str(), port) {
        ("smb" | "cifs", None | Some("445")) if !parts.is_empty() => host.to_owned(),
        ("dav" | "webdav", None | Some("80")) => format!(r"{host}\DavWWWRoot"),
        ("dav" | "webdav", Some(p)) => format!(r"{host}@{p}\DavWWWRoot"),
        ("davs" | "webdavs", None | Some("443")) => format!(r"{host}@SSL\DavWWWRoot"),
        ("davs" | "webdavs", Some(p)) => format!(r"{host}@SSL@{p}\DavWWWRoot"),
        _ => return Err(scheme),
    };
    let mut unc = format!(r"\\{server}");
    for p in &parts {
        unc.push('\\');
        unc.push_str(p);
    }
    FsPath::parse(&unc).filter(|p| p.kind() == PathKind::Unc && !host.is_empty()).ok_or(scheme)
}

pub(crate) fn share_root(dir: &FsPath) -> Option<String> {
    let (server, share, _) = dir.unc_parts()?;
    Some(format!(r"\\{}\{}", String::from_utf8_lossy(server), String::from_utf8_lossy(share)))
}

pub(crate) fn remote(path: &FsPath) -> bool {
    match path.kind() {
        PathKind::Unc | PathKind::Shell => true,
        PathKind::Drive => {
            let root = wide(&format!("{}\\", letter(path).unwrap_or_default()));
            unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_REMOTE }
        }
        _ => false,
    }
}

pub(crate) fn connection(local: &str) -> Option<(String, bool)> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let online = match unsafe { WNetGetConnectionW(PCWSTR(wide(local).as_ptr()), Some(PWSTR(buf.as_mut_ptr())), &mut len) } {
        NO_ERROR => true,
        ERROR_CONNECTION_UNAVAIL => false,
        _ => return None,
    };
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some((String::from_utf16_lossy(&buf[..end]), online)).filter(|(r, _)| !r.is_empty())
}

pub(crate) fn remembered() -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut handle = HANDLE::default();
        if WNetOpenEnumW(RESOURCE_REMEMBERED, RESOURCETYPE_DISK, WNET_OPEN_ENUM_USAGE(0), None, &mut handle) != NO_ERROR {
            return out;
        }
        let mut buf = vec![0u64; 2048];
        loop {
            let (mut count, mut size) = (u32::MAX, (buf.len() * 8) as u32);
            if WNetEnumResourceW(handle, &mut count, buf.as_mut_ptr().cast(), &mut size) != NO_ERROR {
                break;
            }
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const NETRESOURCEW, count as usize);
            for r in items {
                if let (Ok(local), Ok(remote)) = (r.lpLocalName.to_string(), r.lpRemoteName.to_string()) {
                    out.push((local.to_ascii_uppercase(), remote));
                }
            }
        }
        let _ = WNetCloseEnum(handle);
    }
    out
}

static DECLINED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub(crate) fn needs_logon(dir: &FsPath, e: &FsError) -> bool {
    let code = WIN32_ERROR(e.code as u32);
    let logon = matches!(code, ERROR_LOGON_FAILURE | ERROR_NOT_AUTHENTICATED | ERROR_INVALID_PASSWORD) || (code == ERROR_ACCESS_DENIED && dir.is_root());
    logon && share_root(dir).is_some_and(|s| !DECLINED.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashSet::new).contains(&s.to_ascii_lowercase()))
}

pub(crate) fn logon(dir: &FsPath) -> bool {
    let Some(share) = share_root(dir) else { return false };
    let mut remote = wide(&share);
    let res = NETRESOURCEW { dwType: RESOURCETYPE_DISK, lpRemoteName: PWSTR(remote.as_mut_ptr()), ..Default::default() };
    let done = unsafe { WNetAddConnection3W(owner_window(), &res, PCWSTR::null(), PCWSTR::null(), CONNECT_INTERACTIVE) };
    if done != NO_ERROR {
        DECLINED.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashSet::new).insert(share.to_ascii_lowercase());
    }
    done == NO_ERROR
}

fn finished(code: WIN32_ERROR) -> Result<(), PlatformError> {
    match code {
        NO_ERROR | ERROR_CANCELLED => Ok(()),
        c if c.0 == CANCELLED_BY_USER => Ok(()),
        c => Err(err(windows::core::Error::from(c.to_hresult()))),
    }
}

pub(crate) fn map_dialog(prefill: Option<&FsPath>) -> Result<(), PlatformError> {
    let mut remote = prefill.and_then(share_root).map(|s| wide(&s));
    let mut res = NETRESOURCEW { dwType: RESOURCETYPE_DISK, lpRemoteName: remote.as_mut().map_or(PWSTR::null(), |r| PWSTR(r.as_mut_ptr())), ..Default::default() };
    let mut dlg = CONNECTDLGSTRUCTW { cbStructure: size_of::<CONNECTDLGSTRUCTW>() as u32, hwndOwner: owner_window().unwrap_or_default(), lpConnRes: &mut res, dwFlags: CONNDLG_USE_MRU, dwDevNum: 0 };
    finished(unsafe { WNetConnectionDialog1W(&mut dlg) })
}

pub(crate) fn disconnect_dialog(drive: Option<&FsPath>) -> Result<(), PlatformError> {
    let Some(mut local) = drive.and_then(letter).map(|l| wide(&l)) else {
        return finished(unsafe { WNetDisconnectDialog(owner_window(), RESOURCETYPE_DISK.0) });
    };
    let dlg = DISCDLGSTRUCTW { cbStructure: size_of::<DISCDLGSTRUCTW>() as u32, hwndOwner: owner_window().unwrap_or_default(), lpLocalName: PWSTR(local.as_mut_ptr()), lpRemoteName: PWSTR::null(), dwFlags: DISC_UPDATE_PROFILE };
    finished(unsafe { WNetDisconnectDialog1W(&dlg) })
}

pub(crate) fn reconnect(drive: &FsPath) -> Result<FsPath, PlatformError> {
    let local = letter(drive).ok_or_else(|| PlatformError::unsupported("mounting volumes"))?;
    let remote = remembered().into_iter().find(|(l, _)| l.eq_ignore_ascii_case(&local)).map(|(_, r)| r).or_else(|| connection(&local).map(|(r, _)| r)).ok_or_else(|| PlatformError::new(ERROR_CONNECTION_UNAVAIL.0 as i64, drive.display()))?;
    let (mut l, mut r) = (wide(&local), wide(&remote));
    let res = NETRESOURCEW { dwType: RESOURCETYPE_DISK, lpLocalName: PWSTR(l.as_mut_ptr()), lpRemoteName: PWSTR(r.as_mut_ptr()), ..Default::default() };
    match unsafe { WNetAddConnection3W(owner_window(), &res, PCWSTR::null(), PCWSTR::null(), CONNECT_INTERACTIVE) } {
        NO_ERROR => Ok(drive.clone()),
        c => Err(err(windows::core::Error::from(c.to_hresult()))),
    }
}

#[cfg(test)]
#[path = "../test/network.rs"]
mod tests;
