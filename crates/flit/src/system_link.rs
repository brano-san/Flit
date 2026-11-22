use flit_core::i18n::S;
use flit_core::open::{Open, OpenTarget, Route};
use flit_core::ui::App;
use flit_core::Location;
#[cfg(test)]
use flit_core::FsPath;
use std::ffi::OsString;
use std::path::Path;
use std::time::Instant;

pub const HANDLER: &str = "--flit-link";

#[path = "link_uri.rs"]
mod uri;
pub(crate) use uri::{is_link, Link};
use uri::parse;

pub fn cli(args: &[OsString]) -> Option<Result<Open, S>> {
    let route = match args {
        [mode, route] if mode == HANDLER => Some(route),
        [route] if is_link(route.as_encoded_bytes()) => Some(route),
        _ if args.iter().any(|arg| arg == HANDLER || is_link(arg.as_encoded_bytes())) => None,
        _ => return None,
    };

    Some(
        route
            .and_then(|route| route.to_str())
            .and_then(|uri| {
                parse(uri, cfg!(windows), &|_| true)?;
                Some(Open { paths: vec![uri.as_bytes().to_vec()], ..Default::default() })
            })
            .ok_or(S::SettingsPageSystemLinkInvalid),
    )
}

pub fn handles(open: &Open) -> bool {
    open.paths.iter().chain(&open.select).any(|path| is_link(path))
}

pub fn resolve(open: &Open) -> Option<Link> {
    match (open.paths.as_slice(), open.select.is_empty(), open.properties, &open.tag) {
        ([raw], true, false, None) => std::str::from_utf8(raw).ok().and_then(|uri| parse(uri, cfg!(windows), &local_directory)),
        _ => None,
    }
}

pub fn open_in(app: &mut App, link: Option<Link>) {
    if !app.protocol_links {
        return app.status(S::SettingsPageSystemLinkDisabled.t());
    }
    match link {
        Some(Link::Folder(path)) => {
            let route = Route { folder: Location::Local(path), select: None };
            app.dispatch(vec![route], OpenTarget::NewTab, Instant::now());
        }
        Some(Link::SavedSearch(query)) if app.open_saved_search(&query) => {}
        _ => app.status(S::SettingsPageSystemLinkInvalid.t()),
    }
}

fn local_directory(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        let root: Vec<_> = path.as_os_str().encode_wide().take(2).chain([u16::from(b'\\'), 0]).collect();
        let kind = unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) };
        if !matches!(kind, 2 | 3 | 5 | 6) {
            return false;
        }
    }
    let mut part = std::path::PathBuf::new();
    for component in path.components() {
        part.push(component);
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&part) else { return false };
        if !meta.is_dir() {
            return false;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "../test/system_link.rs"]
mod tests;
