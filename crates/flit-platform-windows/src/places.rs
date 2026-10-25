use flit_core::ui::Drive;
use flit_core::FsPath;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;

fn wide_str(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let letter = (b'A' + i) as char;
            let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
            let root = PCWSTR(root.as_ptr());
            let kind = unsafe { GetDriveTypeW(root) };
            let mut label = [0u16; 261];
            let (mut total, mut free) = (0u64, 0u64);
            if kind != DRIVE_REMOTE {
                unsafe {
                    let _ = GetVolumeInformationW(root, Some(&mut label), None, None, None, None);
                    let _ = GetDiskFreeSpaceExW(root, None, Some(&mut total), Some(&mut free));
                }
            }
            let name = match wide_str(&label) {
                l if !l.is_empty() => l,
                _ => match kind {
                    DRIVE_FIXED => "Local Disk",
                    DRIVE_REMOVABLE => "USB Drive",
                    DRIVE_CDROM => "CD Drive",
                    DRIVE_REMOTE => "Network Drive",
                    _ => "Drive",
                }
                .to_owned(),
            };
            let path = FsPath::parse(&format!("{letter}:\\"))?;
            Some(Drive { path, label: format!("{name} ({letter}:)"), total, free })
        })
        .collect()
}

pub fn wsl_distros() -> Vec<String> {
    let mut key = HKEY::default();
    let mut out = Vec::new();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Lxss"), None, KEY_READ, &mut key).is_err() {
            return out;
        }
        for index in 0.. {
            let mut sub = [0u16; 256];
            let mut len = sub.len() as u32;
            if RegEnumKeyExW(key, index, Some(PWSTR(sub.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            let mut name = [0u16; 256];
            let mut size = (name.len() * 2) as u32;
            let read = RegGetValueW(key, PCWSTR(sub.as_ptr()), w!("DistributionName"), RRF_RT_REG_SZ, None, Some(name.as_mut_ptr() as _), Some(&mut size));
            if read.is_ok() {
                let distro = wide_str(&name);
                if !distro.is_empty() && !out.contains(&distro) {
                    out.push(distro);
                }
            }
        }
        let _ = RegCloseKey(key);
    }
    out
}
