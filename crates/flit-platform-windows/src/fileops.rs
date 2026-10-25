use crate::shell::{owner_window, shell_addressable, ShellCom};
use crate::{clipboard, err, take_pwstr};
use flit_core::provider::FileOp;
use flit_core::FsPath;
use flit_platform::{FileOps, OpOutcome, PlatformError};
use std::cell::RefCell;
use windows::core::{s, Interface, HRESULT, PCWSTR};
use windows::Win32::Storage::FileSystem::{CreateDirectoryW, GetFileAttributesW, MoveFileExW, FILE_ATTRIBUTE_DIRECTORY, INVALID_FILE_ATTRIBUTES, MOVE_FILE_FLAGS};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_ALL, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, SW_SHOWNORMAL};
use windows_core::{implement, ComObject, Ref};

#[implement(IFileOperationProgressSink)]
#[derive(Default)]
struct Sink {
    done: RefCell<Vec<(FsPath, FsPath)>>,
}

pub(crate) fn item_path(item: &IShellItem) -> Option<FsPath> {
    unsafe { take_pwstr(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?) }.and_then(|w| FsPath::from_utf16(&w))
}

fn shell_item(p: &FsPath) -> Result<IShellItem, PlatformError> {
    let w = p.to_wide(false);
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(|e| PlatformError::new(e.code().0 as i64, format!("{}: {}", p.display(), e.message())))
}

fn wide_name(name: &[u8]) -> Vec<u16> {
    let mut w = Vec::with_capacity(name.len() + 1);
    flit_core::name::to_utf16(name, &mut w);
    w.push(0);
    w
}

fn same_path(a: &FsPath, b: &FsPath) -> bool {
    a.as_bytes().eq_ignore_ascii_case(b.as_bytes())
}

impl Sink_Impl {
    fn record(&self, source: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) {
        if hr.is_err() {
            return;
        }
        if let (Some(src), Some(new)) = (source.as_ref().and_then(item_path), created.as_ref().and_then(item_path)) {
            self.done.borrow_mut().push((src, new));
        }
    }
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(&self, _: u32, item: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(&self, _: u32, item: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(&self, _: u32, item: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, item: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        match created.as_ref() {
            Some(_) => self.record(item, hr, created),
            None => {
                if let Some(gone) = item.as_ref().and_then(item_path).filter(|_| hr.is_ok()) {
                    self.done.borrow_mut().push((gone.clone(), gone));
                }
            }
        }
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, hr: HRESULT, item: Ref<IShellItem>) -> windows_core::Result<()> {
        if let Some(p) = item.as_ref().and_then(item_path).filter(|_| hr.is_ok()) {
            self.done.borrow_mut().push((p.clone(), p));
        }
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
}

fn check_names(op: &FileOp) -> Result<(), PlatformError> {
    let paths: Vec<&FsPath> = match op {
        FileOp::Trash(v) => v.iter().collect(),
        FileOp::Copy { items, dest } | FileOp::Move { items, dest } | FileOp::Link { items, dest } => items.iter().chain([dest]).collect(),
        FileOp::Rename { item, .. } => vec![item],
        FileOp::NewFolder { dir, .. } => vec![dir],
        FileOp::MoveTo(pairs) | FileOp::Restore(pairs) => pairs.iter().flat_map(|(a, b)| [a, b]).collect(),
        FileOp::Paste { dest } => vec![dest],
    };
    paths.into_iter().try_for_each(shell_addressable)?;
    match op {
        FileOp::Rename { name, .. } | FileOp::NewFolder { name, .. } if name.ends_with(b".") || name.ends_with(b" ") || name.is_empty() => {
            Err(PlatformError::new(-1, "A name cannot be empty or end with a dot or a space"))
        }
        _ => Ok(()),
    }
}

fn sources(op: &FileOp) -> Option<Vec<FsPath>> {
    match op {
        FileOp::Trash(v) | FileOp::Copy { items: v, .. } | FileOp::Move { items: v, .. } => Some(v.clone()),
        FileOp::MoveTo(pairs) => Some(pairs.iter().map(|(a, _)| a.clone()).collect()),
        FileOp::Rename { item, .. } => Some(vec![item.clone()]),
        _ => None,
    }
}

unsafe fn recycled_items(paths: &[FsPath]) -> Result<Vec<Option<IShellItem>>, PlatformError> {
    let bin: IShellItem = SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None).map_err(err)?;
    let items: IEnumShellItems = bin.BindToHandler(None, &BHID_EnumItems).map_err(err)?;
    let mut found: Vec<Option<IShellItem>> = vec![None; paths.len()];
    while found.iter().any(Option::is_none) {
        let mut one = [None];
        let mut n = 0;
        if items.Next(&mut one, Some(&mut n)).is_err() || n == 0 {
            break;
        }
        let Some(item) = one[0].take() else { break };
        if let Some(k) = item_path(&item).and_then(|p| paths.iter().position(|q| same_path(q, &p))) {
            found[k] = Some(item);
        }
    }
    Ok(found)
}

unsafe fn undelete(items: &[IShellItem]) -> Result<(), PlatformError> {
    let bin: IShellItem = SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None).map_err(err)?;
    let folder: IShellFolder = bin.BindToHandler(None, &BHID_SFObject).map_err(err)?;
    let pidls = items.iter().map(|i| SHGetIDListFromObject(i).map_err(err)).collect::<Result<Vec<_>, _>>();
    let result = pidls.as_ref().map_err(Clone::clone).and_then(|pidls| {
        let children: Vec<_> = pidls.iter().map(|&p| ILFindLastID(p) as *const _).collect();
        let owner = owner_window().unwrap_or_default();
        let menu: IContextMenu = folder.GetUIObjectOf(owner, &children, None).map_err(err)?;
        let hmenu = CreatePopupMenu().map_err(err)?;
        let _ = menu.QueryContextMenu(hmenu, 0, 1, 0x7FFF, CMF_NORMAL);
        let info = CMINVOKECOMMANDINFO { cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32, hwnd: owner, lpVerb: s!("undelete"), nShow: SW_SHOWNORMAL.0, ..Default::default() };
        let invoked = menu.InvokeCommand(&info).map_err(err);
        let _ = DestroyMenu(hmenu);
        invoked
    });
    for p in pidls.into_iter().flatten() {
        ILFree(Some(p));
    }
    result
}

unsafe fn restore(pairs: &[(FsPath, FsPath)]) -> OpOutcome {
    let recycled: Vec<FsPath> = pairs.iter().map(|(r, _)| r.clone()).collect();
    let found = match recycled_items(&recycled) {
        Ok(f) => f,
        Err(e) => return OpOutcome { items: Vec::new(), error: Some(e) },
    };
    let items: Vec<IShellItem> = found.iter().flatten().cloned().collect();
    let mut error = match items.len() {
        0 => Some(PlatformError::new(-1, "The deleted items are no longer in the Recycle Bin")),
        n if n < pairs.len() => Some(PlatformError::new(-1, "Some deleted items are no longer in the Recycle Bin")),
        _ => None,
    };
    if !items.is_empty() {
        if let Err(e) = undelete(&items) {
            error = Some(e);
        }
    }
    let items = pairs.iter().zip(&found).filter(|(_, f)| f.is_some()).map(|(p, _)| p.clone()).filter(|(_, original)| exists(original)).collect();
    OpOutcome { items, error }
}

fn parent_and_name(p: &FsPath) -> Result<(IShellItem, Vec<u16>), PlatformError> {
    let parent = p.parent().ok_or_else(|| PlatformError::new(-1, format!("{} has no parent folder", p.display())))?;
    Ok((shell_item(&parent)?, wide_name(p.file_name())))
}

unsafe fn queue(fo: &IFileOperation, op: &FileOp) -> Result<(), PlatformError> {
    match op {
        FileOp::Trash(paths) => {
            for p in paths {
                fo.DeleteItem(&shell_item(p)?, None).map_err(err)?;
            }
        }
        FileOp::Copy { items, dest } => {
            let dest = shell_item(dest)?;
            for p in items {
                fo.CopyItem(&shell_item(p)?, &dest, PCWSTR::null(), None).map_err(err)?;
            }
        }
        FileOp::Move { items, dest } => {
            let dest = shell_item(dest)?;
            for p in items {
                fo.MoveItem(&shell_item(p)?, &dest, PCWSTR::null(), None).map_err(err)?;
            }
        }
        FileOp::MoveTo(pairs) => {
            for (from, to) in pairs {
                let (dir, name) = parent_and_name(to)?;
                fo.MoveItem(&shell_item(from)?, &dir, PCWSTR(name.as_ptr()), None).map_err(err)?;
            }
        }
        FileOp::Rename { item, name } => {
            let name = wide_name(name);
            fo.RenameItem(&shell_item(item)?, PCWSTR(name.as_ptr()), None).map_err(err)?;
        }
        FileOp::NewFolder { dir, name } => {
            let name = wide_name(name);
            fo.NewItem(&shell_item(dir)?, FILE_ATTRIBUTE_DIRECTORY.0, PCWSTR(name.as_ptr()), PCWSTR::null(), None).map_err(err)?;
        }
        FileOp::Link { .. } | FileOp::Restore(_) | FileOp::Paste { .. } => return Err(PlatformError::new(-1, "not a file operation")),
    }
    Ok(())
}

fn flags(op: &FileOp) -> FILEOPERATION_FLAGS {
    match op {
        FileOp::Trash(_) => FOF_ALLOWUNDO | FOFX_ADDUNDORECORD | FOFX_RECYCLEONDELETE,
        FileOp::NewFolder { .. } => FOF_ALLOWUNDO | FOF_RENAMEONCOLLISION,
        FileOp::Copy { items, dest } if items.iter().any(|i| i.parent().is_some_and(|p| same_path(&p, dest))) => FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | FOF_RENAMEONCOLLISION,
        _ => FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR,
    }
}

fn cancelled(op: &FileOp) -> PlatformError {
    let what = if matches!(op, FileOp::Trash(_)) { "Deletion" } else { "The operation" };
    PlatformError::new(-1, format!("{what} was cancelled"))
}

unsafe fn perform(op: &FileOp) -> OpOutcome {
    let sink = ComObject::new(Sink::default());
    let result = (|| {
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(err)?;
        fo.SetOperationFlags(flags(op)).map_err(err)?;
        if let Some(owner) = owner_window() {
            fo.SetOwnerWindow(owner).map_err(err)?;
        }
        queue(&fo, op)?;
        let cookie = fo.Advise(&sink.to_interface::<IFileOperationProgressSink>()).map_err(err)?;
        let performed = fo.PerformOperations();
        let _ = fo.Unadvise(cookie);
        performed.map_err(err)?;
        if fo.GetAnyOperationsAborted().map_err(err)?.as_bool() {
            return Err(cancelled(op));
        }
        Ok(())
    })();
    let mut items = sink.done.take();
    if let Some(wanted) = sources(op) {
        items.retain(|(src, _)| wanted.iter().any(|w| same_path(w, src)));
    }
    OpOutcome { items, error: result.err() }
}

fn exists(p: &FsPath) -> bool {
    unsafe { GetFileAttributesW(PCWSTR(p.to_wide(true).as_ptr())) != INVALID_FILE_ATTRIBUTES }
}

fn is_dir(p: &FsPath) -> bool {
    let attrs = unsafe { GetFileAttributesW(PCWSTR(p.to_wide(true).as_ptr())) };
    attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0
}

fn notify(event: SHCNE_ID, item: &FsPath, other: Option<&FsPath>) {
    let (a, b) = (item.to_wide(false), other.map(|o| o.to_wide(false)));
    unsafe { SHChangeNotify(event, SHCNF_PATHW | SHCNF_FLUSHNOWAIT, Some(a.as_ptr().cast()), b.as_ref().map(|b| b.as_ptr().cast())) };
}

fn direct_move(from: &FsPath, to: &FsPath) -> bool {
    let (src, dst) = (from.to_wide(true), to.to_wide(true));
    if unsafe { MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVE_FILE_FLAGS(0)) }.is_err() {
        return false;
    }
    notify(if is_dir(to) { SHCNE_RENAMEFOLDER } else { SHCNE_RENAMEITEM }, from, Some(to));
    true
}

fn direct(op: &FileOp) -> (Vec<(FsPath, FsPath)>, Option<FileOp>) {
    let pairs: Vec<(FsPath, FsPath)> = match op {
        FileOp::Move { items, dest } => items.iter().map(|i| (i.clone(), dest.join(i.file_name()))).collect(),
        FileOp::MoveTo(pairs) => pairs.clone(),
        FileOp::Rename { item, name } => match item.parent() {
            Some(dir) => vec![(item.clone(), dir.join(name))],
            None => return (Vec::new(), Some(op.clone())),
        },
        FileOp::NewFolder { dir, name } => {
            let path = dir.join(name);
            if unsafe { CreateDirectoryW(PCWSTR(path.to_wide(true).as_ptr()), None) }.is_err() {
                return (Vec::new(), Some(op.clone()));
            }
            notify(SHCNE_MKDIR, &path, None);
            return (vec![(path.clone(), path)], None);
        }
        _ => return (Vec::new(), Some(op.clone())),
    };
    let mut done = Vec::new();
    for (from, to) in &pairs {
        if !direct_move(from, to) {
            break;
        }
        done.push((from.clone(), to.clone()));
    }
    let rest = match done.len() {
        0 => Some(op.clone()),
        n if n < pairs.len() => Some(FileOp::MoveTo(pairs[n..].to_vec())),
        _ => None,
    };
    (done, rest)
}

fn link_path(dest: &FsPath, target: &FsPath) -> FsPath {
    let stem: Vec<u8> = target.file_name().iter().copied().filter(|&c| c != b':' && c != b'\\').collect();
    (1..)
        .map(|k| {
            let mut name = stem.clone();
            name.extend_from_slice(b" - Shortcut");
            if k > 1 {
                name.extend_from_slice(format!(" ({k})").as_bytes());
            }
            name.extend_from_slice(b".lnk");
            dest.join(&name)
        })
        .find(|p| !exists(p))
        .unwrap()
}

unsafe fn make_links(items: &[FsPath], dest: &FsPath) -> OpOutcome {
    let mut out = OpOutcome::default();
    for target in items {
        let made = (|| {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
            link.SetPath(PCWSTR(target.to_wide(false).as_ptr())).map_err(err)?;
            if let Some(dir) = target.parent() {
                link.SetWorkingDirectory(PCWSTR(dir.to_wide(false).as_ptr())).map_err(err)?;
            }
            let path = link_path(dest, target);
            link.cast::<IPersistFile>().map_err(err)?.Save(PCWSTR(path.to_wide(false).as_ptr()), true).map_err(err)?;
            Ok::<_, PlatformError>(path)
        })();
        match made {
            Ok(path) => out.items.push((target.clone(), path)),
            Err(e) => {
                out.error = Some(e);
                break;
            }
        }
    }
    out
}

impl FileOps for ShellCom {
    fn file_op(&mut self, op: &FileOp) -> OpOutcome {
        if let Err(e) = check_names(op) {
            return OpOutcome { items: Vec::new(), error: Some(e) };
        }
        match op {
            FileOp::Link { items, dest } => unsafe { make_links(items, dest) },
            FileOp::Restore(pairs) => unsafe { restore(pairs) },
            _ => {
                let (mut items, rest) = direct(op);
                let Some(rest) = rest else { return OpOutcome { items, error: None } };
                let out = unsafe { perform(&rest) };
                items.extend(out.items);
                OpOutcome { items, error: out.error }
            }
        }
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        clipboard::read_files()
    }

    fn clear_clipboard(&mut self) {
        clipboard::clear_if_unchanged();
    }
}

#[cfg(test)]
#[path = "../test/fileops.rs"]
mod tests;
