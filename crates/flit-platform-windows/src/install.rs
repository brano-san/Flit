use flit_core::i18n::S;
use flit_core::provider::Stage;
use flit_core::{install, tr_en, FsPath};
use flit_platform::{Job, OpOutcome, PlatformError};
use std::path::{Path, PathBuf};
use windows::core::{HSTRING, PCWSTR};
use windows::Storage::StorageFile;
use windows::System::UserProfile::LockScreen;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::AddFontResourceW;
use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_FONTCHANGE};

const FONTS_KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Fonts";

pub(crate) fn user_fonts_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Fonts"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn font_error(src: &FsPath, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(-1, tr_en!(S::ErrorsOpsExtraFont, path = src.display(), error = error))
}

pub(crate) fn copy_font(src: &FsPath, dir: &Path) -> Result<(PathBuf, String), PlatformError> {
    let value = install::font_value_name(src.file_name()).ok_or_else(|| PlatformError::new(-1, tr_en!(S::ErrorsOpsExtraNotFont, path = src.display())))?;
    let to = dir.join(crate::fileops::std_path(src).file_name().unwrap_or_default());
    std::fs::create_dir_all(dir).map_err(|e| font_error(src, e))?;
    std::fs::copy(crate::fileops::std_path(src), &to).map_err(|e| font_error(src, e))?;
    Ok((to, value))
}

fn register_font(src: &FsPath, file: &Path, value: &str) -> Result<(), PlatformError> {
    let data = wide(&file.to_string_lossy());
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
    let set = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(wide(FONTS_KEY).as_ptr()), PCWSTR(wide(value).as_ptr()), REG_SZ.0, Some(bytes.as_ptr() as *const _), bytes.len() as u32) };
    set.ok().map_err(|e| font_error(src, e))?;
    unsafe { AddFontResourceW(PCWSTR(data.as_ptr())) };
    Ok(())
}

pub(crate) fn install_fonts(fonts: &[FsPath], job: &Job) -> OpOutcome {
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_items = fonts.len() as u64;
    });
    let (mut items, mut error) = (Vec::new(), None);
    let Some(dir) = user_fonts_dir() else { return OpOutcome { items, error: Some(PlatformError::new(-1, tr_en!(S::ErrorsOpsExtraUnsupported))) } };
    for src in fonts {
        if job.cancelled() {
            break;
        }
        match copy_font(src, &dir).and_then(|(to, value)| register_font(src, &to, &value).map(|()| to)) {
            Ok(to) => items.push((src.clone(), FsPath::parse(&to.to_string_lossy()).unwrap_or_else(|| src.clone()))),
            Err(e) => error = error.or(Some(e)),
        }
        job.update(|p| p.items += 1);
    }
    if !items.is_empty() {
        unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_FONTCHANGE, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 1000, None) };
    }
    OpOutcome { items, error }
}

pub(crate) fn lock_screen(image: &FsPath) -> OpOutcome {
    let set = || -> windows::core::Result<()> {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(crate::fileops::std_path(image).as_os_str()))?.join()?;
        LockScreen::SetImageFileAsync(&file)?.join()
    };
    match set() {
        Ok(()) => OpOutcome { items: vec![(image.clone(), image.clone())], error: None },
        Err(e) => OpOutcome { items: Vec::new(), error: Some(PlatformError::new(e.code().0 as i64, tr_en!(S::ErrorsOpsExtraLockScreen, error = e.message()))) },
    }
}

#[cfg(test)]
#[path = "../test/install.rs"]
mod tests;
