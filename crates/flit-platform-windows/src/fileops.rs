use crate::engine;
use crate::recycle::{self, info_file};
use crate::shell::{owner_window, shell_addressable, ShellCom};
use crate::{clipboard, err, take_pwstr};
use flit_core::listing::attr;
use flit_core::provider::{FileOp, Stage, Template};
use flit_core::FsPath;
use flit_core::i18n::S;
use flit_core::tr_en;
use flit_platform::{FileOps, Job, OpOutcome, PlatformError};
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::Write;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::core::{s, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::Storage::FileSystem::{
    CopyFile2, CreateDirectoryW, DeleteFileW, GetFileAttributesW, MoveFileExW, SetFileAttributesW, COPYFILE2_EXTENDED_PARAMETERS, COPY_FILE_FAIL_IF_EXISTS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_READONLY, FILE_FLAGS_AND_ATTRIBUTES, INVALID_FILE_ATTRIBUTES, MOVEFILE_REPLACE_EXISTING, MOVE_FILE_FLAGS,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, SW_SHOWNORMAL};
use windows_core::{implement, ComObject, Ref};

#[implement(IFileOperationProgressSink)]
struct Sink {
    done: RefCell<Vec<(FsPath, FsPath)>>,
    existing: RefCell<Vec<(FsPath, FsPath)>>,
    job: Job,
    path: fn(&IShellItem) -> Option<FsPath>,
}

pub(crate) fn item_path(item: &IShellItem) -> Option<FsPath> {
    unsafe { take_pwstr(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?) }.and_then(|w| FsPath::from_utf16(&w))
}

fn operation_item_path(item: &IShellItem) -> Option<FsPath> {
    item_path(item).or_else(|| {
        [SIGDN_URL, SIGDN_DESKTOPABSOLUTEPARSING].into_iter().find_map(|kind| {
            let uri = unsafe { take_pwstr(item.GetDisplayName(kind).ok()?) }?;
            crate::network::server_path(&String::from_utf16(&uri).ok()?).ok().filter(FsPath::is_ftp)
        })
    })
}

fn shell_item(p: &FsPath) -> Result<IShellItem, PlatformError> {
    if p.kind() == flit_core::path::PathKind::Shell {
        return crate::namespace::item(p).map_err(|e| PlatformError::new(e.code, format!("{}: {e}", p.display())));
    }
    let w = p.to_wide(false);
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(|e| crate::win_error(e.code().0 as i64, format!("{}: {}", p.display(), crate::error_reason(&e))))
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
    fn go_on(&self) -> windows_core::Result<()> {
        match self.job.cancelled() {
            true => Err(ERROR_CANCELLED.to_hresult().into()),
            false => Ok(()),
        }
    }

    fn before_transfer(&self, item: Ref<IShellItem>, dir: Ref<IShellItem>, name: &PCWSTR) -> windows_core::Result<()> {
        self.go_on()?;
        if let (Some(source), Some(dir)) = (item.as_ref().and_then(item_path), dir.as_ref().and_then(item_path)) {
            let mut leaf = Vec::new();
            if name.is_null() {
                leaf.extend_from_slice(source.file_name());
            } else {
                flit_core::name::push_utf16(&mut leaf, unsafe { name.as_wide() });
            }
            let target = dir.join(&leaf);
            if !same_path(&source, &target) && engine::stat(&target).is_some_and(|meta| meta.attrs & attr::DIR == 0) {
                self.existing.borrow_mut().push((source, target));
            }
        }
        Ok(())
    }

    fn after_transfer(&self, item: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) {
        if let Some(source) = item.as_ref().and_then(item_path) {
            let mut existing = self.existing.borrow_mut();
            if let Some(index) = existing.iter().position(|(path, _)| same_path(path, &source)) {
                let (_, before) = existing.swap_remove(index);
                if hr.is_ok() && created.as_ref().and_then(item_path).is_some_and(|target| same_path(&before, &target)) {
                    self.job.replaced(before, None);
                }
            }
        }
        self.record(item, hr, created);
    }

    fn record(&self, source: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) {
        if hr.is_err() {
            return;
        }
        if let Some(new) = created.as_ref().and_then(self.path) {
            let src = source.as_ref().and_then(self.path).unwrap_or_else(|| new.clone());
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
        self.go_on()
    }
    fn PostRenameItem(&self, _: u32, item: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, item: Ref<IShellItem>, dir: Ref<IShellItem>, name: &PCWSTR) -> windows_core::Result<()> {
        self.before_transfer(item, dir, name)
    }
    fn PostMoveItem(&self, _: u32, item: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        self.after_transfer(item, hr, created);
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, item: Ref<IShellItem>, dir: Ref<IShellItem>, name: &PCWSTR) -> windows_core::Result<()> {
        self.before_transfer(item, dir, name)
    }
    fn PostCopyItem(&self, _: u32, item: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows_core::Result<()> {
        self.after_transfer(item, hr, created);
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> windows_core::Result<()> {
        self.go_on()
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
    fn UpdateProgress(&self, total: u32, done: u32) -> windows_core::Result<()> {
        self.job.update(|p| {
            p.stage = Stage::Running;
            p.total_items = total.into();
            p.items = done.into();
        });
        self.go_on()
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
        FileOp::Trash(v) | FileOp::Delete(v) | FileOp::Flatten(v) => v.iter().collect(),
        FileOp::Purge(pairs) => pairs.iter().map(|(r, _)| r).collect(),
        FileOp::EmptyBin => Vec::new(),
        FileOp::Copy { items, dest } | FileOp::Move { items, dest } | FileOp::Link { items, dest } | FileOp::LinkAs { items, dest, .. } => items.iter().chain([dest]).collect(),
        FileOp::Rename { item, .. } => vec![item],
        FileOp::NewFolder { dir, .. } | FileOp::NewFile { dir, .. } => vec![dir],
        FileOp::NewStream { item, .. } => vec![item],
        FileOp::FolderIcon { folder, icon, .. } => [folder].into_iter().chain(icon).collect(),
        FileOp::MoveTo(pairs) | FileOp::Restore(pairs) => pairs.iter().flat_map(|(a, b)| [a, b]).collect(),
        FileOp::Paste { dest } | FileOp::PasteLink { dest } | FileOp::PasteLinkAs { dest, .. } | FileOp::PasteDropped { dest, .. } => vec![dest],
        FileOp::Archive(_) => Vec::new(),
        FileOp::Segments { source, dest, .. } => vec![source, dest],
        FileOp::Dedup { keep, candidates, .. } => [keep].into_iter().chain(candidates).collect(),
        FileOp::RestoreDedup { keep, backups } => [keep].into_iter().chain(backups.iter().flat_map(|b| [&b.target, &b.trash])).collect(),
        FileOp::InstallFonts(v) => v.iter().collect(),
        FileOp::LockScreen(p) | FileOp::GrantAccess(p) => vec![p],
        FileOp::SetAccess { items, .. } | FileOp::SetAttributes { items, .. } => items.iter().collect(),
        FileOp::Access(changes) => changes.iter().map(|c| &c.item).collect(),
    };
    paths.into_iter().try_for_each(shell_addressable)?;
    match op {
        FileOp::Rename { name, .. } | FileOp::NewFolder { name, .. } | FileOp::NewFile { name, .. } if name.ends_with(b".") || name.ends_with(b" ") || name.is_empty() => {
            Err(PlatformError::new(-1, S::ErrorsEngineBadName.en()))
        }
        _ => Ok(()),
    }
}

fn sources(op: &FileOp) -> Option<Vec<FsPath>> {
    match op {
        FileOp::Trash(v) | FileOp::Delete(v) | FileOp::Copy { items: v, .. } | FileOp::Move { items: v, .. } => Some(v.clone()),
        FileOp::MoveTo(pairs) => Some(pairs.iter().map(|(a, _)| a.clone()).collect()),
        FileOp::Rename { item, .. } => Some(vec![item.clone()]),
        _ => None,
    }
}

pub(crate) unsafe fn recycled_items(paths: &[FsPath]) -> Result<Vec<Option<IShellItem>>, PlatformError> {
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
        0 => Some(PlatformError::new(-1, S::ErrorsEngineRestoreGone.en())),
        n if n < pairs.len() => Some(PlatformError::new(-1, S::ErrorsEngineRestoreSomeGone.en())),
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
    let parent = p.parent().ok_or_else(|| PlatformError::new(-1, tr_en!(S::ErrorsEngineNoParent, path = p.display())))?;
    Ok((shell_item(&parent)?, wide_name(p.file_name())))
}

unsafe fn queue(fo: &IFileOperation, op: &FileOp) -> Result<(), PlatformError> {
    match op {
        FileOp::Trash(paths) | FileOp::Delete(paths) => {
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
        FileOp::NewFile { dir, name, template } => {
            let name = wide_name(name);
            let source = match template {
                Template::Copy(file) => Some(wide_name(file.file_name())),
                _ => None,
            };
            fo.NewItem(&shell_item(dir)?, FILE_ATTRIBUTE_NORMAL.0, PCWSTR(name.as_ptr()), source.as_ref().map_or(PCWSTR::null(), |s| PCWSTR(s.as_ptr())), None).map_err(err)?;
        }
        FileOp::Link { .. } | FileOp::LinkAs { .. } | FileOp::PasteLinkAs { .. } | FileOp::Restore(_) | FileOp::Paste { .. } | FileOp::PasteLink { .. } | FileOp::Purge(_) | FileOp::EmptyBin | FileOp::NewStream { .. } | FileOp::Flatten(_) | FileOp::FolderIcon { .. } | FileOp::Archive(_) | FileOp::InstallFonts(_) | FileOp::LockScreen(_) | FileOp::SetAccess { .. } | FileOp::Access(_) | FileOp::SetAttributes { .. } | FileOp::PasteDropped { .. } | FileOp::GrantAccess(_) | FileOp::Segments { .. } | FileOp::Dedup { .. } | FileOp::RestoreDedup { .. } => return Err(PlatformError::new(-1, "not a file operation")),
    }
    Ok(())
}

const QUIET_ENTRIES: usize = 500;
const QUIET_BYTES: u64 = 16 << 20;

pub(crate) fn std_path(p: &FsPath) -> PathBuf {
    let mut w = p.to_wide(true);
    w.pop();
    PathBuf::from(OsString::from_wide(&w))
}

fn fits(path: &Path, meta: std::fs::Metadata, left: &mut (usize, u64)) -> bool {
    match (left.0.checked_sub(1), left.1.checked_sub(meta.len())) {
        (Some(entries), Some(bytes)) => *left = (entries, bytes),
        _ => return false,
    }
    !meta.is_dir() || std::fs::read_dir(path).is_ok_and(|mut dir| dir.all(|e| e.is_ok_and(|e| e.metadata().is_ok_and(|m| fits(&e.path(), m, left)))))
}

fn small(p: &FsPath, files_only: bool, left: &mut (usize, u64)) -> bool {
    let p = std_path(p);
    std::fs::symlink_metadata(&p).is_ok_and(|m| (m.is_file() || !files_only) && fits(&p, m, left))
}

fn quiet(op: &FileOp) -> bool {
    let mut left = (QUIET_ENTRIES, QUIET_BYTES);
    sources(op).is_none_or(|paths| paths.iter().all(|p| small(p, false, &mut left)))
}

fn duplicates(items: &[FsPath], dest: &FsPath) -> bool {
    items.iter().any(|i| i.parent().is_some_and(|p| same_path(&p, dest)))
}

fn flags(op: &FileOp) -> FILEOPERATION_FLAGS {
    let flags = match op {
        FileOp::Trash(_) => FOF_ALLOWUNDO | FOFX_ADDUNDORECORD | FOFX_RECYCLEONDELETE,
        FileOp::Delete(_) => FILEOPERATION_FLAGS(0),
        FileOp::NewFolder { .. } | FileOp::NewFile { .. } => FOF_ALLOWUNDO | FOF_RENAMEONCOLLISION,
        FileOp::Copy { items, dest } if duplicates(items, dest) => FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | FOF_RENAMEONCOLLISION,
        _ => FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR,
    };
    if quiet(op) { flags | FOF_SILENT } else { flags }
}

fn cancelled(op: &FileOp) -> PlatformError {
    let what = if matches!(op, FileOp::Trash(_) | FileOp::Delete(_) | FileOp::Purge(_) | FileOp::EmptyBin) { S::ErrorsEngineDeletionCancelled } else { S::ErrorsEngineCancelled };
    PlatformError::cancelled(-1, what.en())
}

unsafe fn operate(flags: FILEOPERATION_FLAGS, fill: impl FnOnce(&IFileOperation) -> Result<(), PlatformError>, cancelled: impl Fn() -> PlatformError, job: &Job, ftp: bool) -> OpOutcome {
    let path: fn(&IShellItem) -> Option<FsPath> = if ftp { operation_item_path } else { item_path };
    let sink = ComObject::new(Sink { done: RefCell::default(), existing: RefCell::default(), job: job.clone(), path });
    let result = (|| {
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(err)?;
        fo.SetOperationFlags(flags).map_err(err)?;
        if let Some(owner) = owner_window() {
            fo.SetOwnerWindow(owner).map_err(err)?;
        }
        fill(&fo)?;
        let cookie = fo.Advise(&sink.to_interface::<IFileOperationProgressSink>()).map_err(err)?;
        let performed = fo.PerformOperations();
        let _ = fo.Unadvise(cookie);
        let aborted = fo.GetAnyOperationsAborted();
        if job.cancelled() || aborted.as_ref().is_ok_and(|value| value.as_bool()) {
            return Err(cancelled());
        }
        performed.map_err(err)?;
        let _ = aborted.map_err(err)?;
        Ok(())
    })();
    OpOutcome { items: sink.done.take(), error: result.err() }
}

unsafe fn perform(op: &FileOp, job: &Job) -> OpOutcome {
    let ftp = !op.all_paths(|path| !path.is_ftp());
    let mut out = operate(flags(op), |fo| queue(fo, op), || cancelled(op), job, ftp);
    if let Some(wanted) = sources(op) {
        out.items.retain(|(src, _)| wanted.iter().any(|w| same_path(w, src)));
    }
    out
}

pub(crate) unsafe fn transfer_items(items: &[IShellItem], dest: &FsPath, moving: bool, job: &Job) -> OpOutcome {
    let fill = |fo: &IFileOperation| {
        let dest = shell_item(dest)?;
        for item in items {
            match moving {
                true => fo.MoveItem(item, &dest, PCWSTR::null(), None),
                false => fo.CopyItem(item, &dest, PCWSTR::null(), None),
            }
            .map_err(err)?;
        }
        Ok(())
    };
    operate(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR, fill, || PlatformError::cancelled(-1, S::ErrorsEngineCancelled.en()), job, dest.is_ftp())
}

pub(crate) fn exists(p: &FsPath) -> bool {
    unsafe { GetFileAttributesW(PCWSTR(p.to_wide(true).as_ptr())) != INVALID_FILE_ATTRIBUTES }
}

pub(crate) fn is_dir(p: &FsPath) -> bool {
    let attrs = unsafe { GetFileAttributesW(PCWSTR(p.to_wide(true).as_ptr())) };
    attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0
}

fn notify(event: SHCNE_ID, item: &FsPath, other: Option<&FsPath>) {
    let (a, b) = (item.to_wide(false), other.map(|o| o.to_wide(false)));
    unsafe { SHChangeNotify(event, SHCNF_PATHW | SHCNF_FLUSHNOWAIT, Some(a.as_ptr().cast()), b.as_ref().map(|b| b.as_ptr().cast())) };
}

fn delete_file(p: &FsPath) {
    let _ = unsafe { DeleteFileW(PCWSTR(p.to_wide(true).as_ptr())) };
}

pub(crate) fn rename_local_noreplace(from: &FsPath, to: &FsPath) -> Result<(), PlatformError> {
    let (src, dst) = (from.to_wide(true), to.to_wide(true));
    unsafe { MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVE_FILE_FLAGS(0)) }.map_err(err)?;
    notify(if is_dir(to) { SHCNE_RENAMEFOLDER } else { SHCNE_RENAMEITEM }, from, Some(to));
    Ok(())
}

fn create_new(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::File::create_new(path)?.write_all(data)
}

fn new_file(path: &FsPath, template: &Template) -> bool {
    let (src, dst) = match template {
        Template::Empty => return create_new(&std_path(path), &[]).is_ok(),
        Template::Data(data) => return create_new(&std_path(path), data).is_ok(),
        Template::Copy(file) => (file.to_wide(true), path.to_wide(true)),
    };
    let params = COPYFILE2_EXTENDED_PARAMETERS { dwSize: size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32, dwCopyFlags: COPY_FILE_FAIL_IF_EXISTS, ..Default::default() };
    unsafe { CopyFile2(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), Some(&params)) }.is_ok() && std::fs::File::options().write(true).open(std_path(path)).and_then(|f| f.set_modified(std::time::SystemTime::now())).is_ok()
}

fn new_stream(item: &FsPath, name: &[u8]) -> OpOutcome {
    let mut w = item.to_wide(true);
    w.pop();
    w.push(u16::from(b':'));
    flit_core::name::to_utf16(name, &mut w);
    match create_new(Path::new(&OsString::from_wide(&w)), &[]) {
        Ok(()) => OpOutcome { items: vec![(item.clone(), item.clone())], error: None },
        Err(e) => OpOutcome { items: Vec::new(), error: Some(PlatformError::from_io(&e, tr_en!(S::ErrorsEngineStream, error = flit_core::fmt::io_reason(&e)))) },
    }
}

fn folder_icon(folder: &FsPath, icon: Option<&FsPath>, index: i32) -> OpOutcome {
    let (dir, mut file) = (folder.to_wide(false), icon.map(|i| i.to_wide(false)));
    let icon_file = file.as_mut().map_or(PWSTR::null(), |f| PWSTR(f.as_mut_ptr()));
    let mut fcs = SHFOLDERCUSTOMSETTINGS { dwSize: size_of::<SHFOLDERCUSTOMSETTINGS>() as u32, dwMask: FCSM_ICONFILE, pszIconFile: icon_file, iIconIndex: index, ..Default::default() };
    match unsafe { SHGetSetFolderCustomSettings(&mut fcs, PCWSTR(dir.as_ptr()), FCS_FORCEWRITE) } {
        Ok(()) => OpOutcome { items: vec![(folder.clone(), icon.unwrap_or(folder).clone())], error: None },
        Err(e) => OpOutcome { items: Vec::new(), error: Some(err(e)) },
    }
}

pub(crate) fn direct_restore(recycled: &FsPath, original: &FsPath) -> bool {
    let Some(info) = info_file(recycled) else { return false };
    if rename_local_noreplace(recycled, original).is_err() {
        return false;
    }
    delete_file(&info);
    true
}

pub(crate) fn restore_checked(recycled: &FsPath, original: &FsPath) -> bool {
    recycle::is_original(recycled, original)
        && original.parent().is_some_and(|dir| std::fs::create_dir_all(std_path(&dir)).is_ok())
        && direct_restore(recycled, original)
}

fn direct(op: &FileOp) -> (Vec<(FsPath, FsPath)>, Option<FileOp>) {
    let created = |path: FsPath, event| {
        notify(event, &path, None);
        (vec![(path.clone(), path)], None)
    };
    match op {
        FileOp::Restore(pairs) => {
            let (done, left): (Vec<_>, Vec<_>) = pairs.iter().cloned().partition(|(recycled, original)| direct_restore(recycled, original));
            (done, (!left.is_empty()).then_some(FileOp::Restore(left)))
        }
        FileOp::Trash(paths) => {
            let (done, left) = recycle::trash(paths);
            for (original, _) in &done {
                notify(SHCNE_DELETE, original, None);
            }
            if !done.is_empty() {
                unsafe { SHUpdateRecycleBinIcon() };
            }
            (done, (!left.is_empty()).then_some(FileOp::Trash(left)))
        }
        FileOp::NewFolder { dir, name } if unsafe { CreateDirectoryW(PCWSTR(dir.join(name).to_wide(true).as_ptr()), None) }.is_ok() => created(dir.join(name), SHCNE_MKDIR),
        FileOp::NewFile { dir, name, template } if new_file(&dir.join(name), template) => created(dir.join(name), SHCNE_CREATE),
        _ => (Vec::new(), Some(op.clone())),
    }
}

const SETTABLE: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x100 | 0x1000 | 0x2000;

pub(crate) fn win_attributes(now: u32, set: u32, clear: u32) -> u32 {
    let bits = |a: u32| (if a & attr::READONLY != 0 { FILE_ATTRIBUTE_READONLY.0 } else { 0 }) | if a & attr::HIDDEN != 0 { FILE_ATTRIBUTE_HIDDEN.0 } else { 0 };
    let next = ((now | bits(set)) & !bits(clear)) & SETTABLE;
    if next == 0 { FILE_ATTRIBUTE_NORMAL.0 } else { next }
}

fn zone_stream(item: &FsPath) -> PathBuf {
    let mut s = std_path(item).into_os_string();
    s.push(":Zone.Identifier");
    PathBuf::from(s)
}

fn change_attributes(item: &FsPath, set: u32, clear: u32, unblock: bool) -> Result<(), PlatformError> {
    let wide = item.to_wide(true);
    let now = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
    if now == INVALID_FILE_ATTRIBUTES {
        return Err(err(windows::core::Error::from_thread()));
    }
    let apply = |a: u32| unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a)) }.map_err(err);
    if unblock {
        if now & FILE_ATTRIBUTE_READONLY.0 != 0 {
            apply(win_attributes(now, 0, attr::READONLY))?;
        }
        if let Err(e) = std::fs::remove_file(zone_stream(item)).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) {
            return Err(PlatformError::from_io(&e, format!("{}: {}", item.display(), flit_core::fmt::io_reason(&e))));
        }
    }
    let next = win_attributes(now, set, clear);
    if next != win_attributes(now, 0, 0) || unblock {
        apply(next)?;
    }
    Ok(())
}

fn set_attributes(items: &[FsPath], set: u32, clear: u32, unblock: bool) -> OpOutcome {
    let mut out = OpOutcome::default();
    for item in items {
        match change_attributes(item, set, clear, unblock) {
            Ok(()) => out.items.push((item.clone(), item.clone())),
            Err(e) => {
                out.error = Some(e);
                break;
            }
        }
    }
    out
}

windows_core::link!("shell32.dll" "system" fn SHUpdateRecycleBinIcon());

fn purge(pairs: &[(FsPath, FsPath)]) -> OpOutcome {
    let out = recycle::purge(pairs);
    unsafe { SHUpdateRecycleBinIcon() };
    out
}

fn empty_bin() -> OpOutcome {
    let roots = recycle::roots();
    if recycle::count(&roots) == 0 {
        return OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, S::ErrorsEngineBinEmpty.en())) };
    }
    let out = recycle::empty(&roots);
    unsafe { SHUpdateRecycleBinIcon() };
    out
}

fn with_rest(mut items: Vec<(FsPath, FsPath)>, error: Option<PlatformError>, rest: Option<FileOp>, job: &Job) -> OpOutcome {
    if rest.is_some() {
        job.set_copy_limit_available(false);
    }
    let out = match rest {
        None => return OpOutcome { items, error },
        Some(FileOp::Restore(pairs)) => unsafe { restore(&pairs) },
        Some(rest) => unsafe { perform(&rest, job) },
    };
    items.extend(out.items);
    OpOutcome { items, error: error.or(out.error) }
}

fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    unsafe { windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

fn device_op(op: &FileOp, job: &Job) -> OpOutcome {
    match op {
        FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Delete(_) | FileOp::Trash(_) | FileOp::Rename { .. } | FileOp::NewFolder { .. } => unsafe { perform(op, job) },
        _ => OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, S::ErrorsFsDeviceUnsupported.en())) },
    }
}

pub(crate) fn verify_dedup_data(path: &FsPath) -> Result<(), PlatformError> {
    use windows::Win32::Foundation::{ERROR_HANDLE_EOF, WIN32_ERROR};
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard, WIN32_FIND_STREAM_DATA};

    let wide = path.to_wide(true);
    let mut data = WIN32_FIND_STREAM_DATA::default();
    let handle = unsafe { FindFirstStreamW(PCWSTR(wide.as_ptr()), FindStreamInfoStandard, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(), None) };
    let handle = match handle {
        Ok(h) => h,
        Err(e) if WIN32_ERROR::from_error(&e) == Some(ERROR_HANDLE_EOF) => return Ok(()),
        Err(e) => return Err(err(e)),
    };
    let result = loop {
        let wide = data.cStreamName.split(|&c| c == 0).next().unwrap_or_default();
        let name = String::from_utf16_lossy(wide);
        if !name.eq_ignore_ascii_case("::$DATA") && !name.eq_ignore_ascii_case(":Zone.Identifier:$DATA") {
            break Err(PlatformError::new(-1, tr_en!(S::DedupRichDataNamed, name = name)));
        }
        match unsafe { FindNextStreamW(handle, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) } {
            Ok(()) => {}
            Err(e) if WIN32_ERROR::from_error(&e) == Some(ERROR_HANDLE_EOF) => break Ok(()),
            Err(e) => break Err(err(e)),
        }
    };
    let _ = unsafe { FindClose(handle) };
    result
}

impl FileOps for ShellCom {
    fn reviews_transfer_conflicts() -> bool {
        true
    }

    fn decode_local_ansi(&self, bytes: &[u8]) -> Option<String> {
        use windows::Win32::Globalization::{CP_ACP, MB_ERR_INVALID_CHARS, MultiByteToWideChar};
        let size = unsafe { MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, bytes, None) };
        if size <= 0 { return None; }
        let mut wide = vec![0; size as usize];
        let decoded = unsafe { MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, bytes, Some(&mut wide)) };
        (decoded == size).then(|| String::from_utf16_lossy(&wide))
    }

    fn rename_local_noreplace(&mut self, from: &FsPath, to: &FsPath) -> Result<(), PlatformError> {
        rename_local_noreplace(from, to)
    }

    fn verify_dedup_data(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        verify_dedup_data(path)
    }

    fn file_identity(&mut self, path: &FsPath) -> Option<flit_core::tags::FileKey> {
        crate::tags::file_key(path)
    }

    fn rename_local_replace(&mut self, from: &FsPath, to: &FsPath) -> Result<(), PlatformError> {
        let (src, dst) = (from.to_wide(true), to.to_wide(true));
        unsafe { MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVEFILE_REPLACE_EXISTING) }.map_err(err)?;
        notify(SHCNE_UPDATEITEM, to, None);
        Ok(())
    }

    fn keep_dedup_backup(&mut self, path: &FsPath, original: &FsPath) -> Option<FsPath> {
        crate::recycle::keep_dedup_backup(path, original)
    }

    fn undo_fs(&self) -> Option<flit_platform::replay::FileSystem> {
        Some(engine::undo_fs())
    }

    fn can_restore(&self, backup: &FsPath, original: &FsPath) -> bool {
        recycle::is_original(backup, original)
    }

    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
        if !op.all_paths(|p| p.kind() != flit_core::path::PathKind::Shell) {
            return device_op(op, job);
        }
        let env = flit_archive::Env { utc_offset: <crate::Windows as flit_platform::SystemPlatform>::utc_offset_minutes(), free_space: &free_space };
        if let Some(out) = flit_archive::file_op(op, job, &env) {
            return out;
        }
        if let Err(e) = check_names(op) {
            return OpOutcome { items: Vec::new(), error: Some(e) };
        }
        crate::prevhost::release(crate::prevhost::releasing(op));
        if job.ctl().replay().is_some() {
            return checked_op(op, job, &SystemRecycle);
        }
        match op {
            FileOp::NewStream { item, name } => new_stream(item, name),
            FileOp::FolderIcon { folder, icon, index } => folder_icon(folder, icon.as_ref(), *index),
            FileOp::Purge(pairs) => purge(pairs),
            FileOp::EmptyBin => empty_bin(),
            FileOp::InstallFonts(fonts) => crate::install::install_fonts(fonts, job),
            FileOp::LockScreen(image) => crate::install::lock_screen(image),
            FileOp::Paste { dest } => crate::transfer::paste(dest, job),
            FileOp::PasteDropped { dest, moving, .. } => crate::transfer::paste_dropped(dest, *moving, job),
            FileOp::GrantAccess(dir) => crate::volume::grant_access(dir),
            FileOp::SetAccess { .. } | FileOp::Access(_) => OpOutcome { items: Vec::new(), error: Some(PlatformError::unsupported("changing Unix permissions")) },
            FileOp::SetAttributes { items, set, clear, unblock } => set_attributes(items, *set, *clear, *unblock),
            op if engine::native(op) => {
                let out = engine::run(op, job);
                let engine_items = out.items.len();
                let unknown = matches!(&out.rest, Some(FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_)));
                let outcome = with_rest(out.items, out.error, out.rest, job);
                if unknown && outcome.items.len() > engine_items {
                    job.mark_unverified();
                }
                outcome
            }
            _ => {
                let (items, rest) = direct(op);
                with_rest(items, None, rest, job)
            }
        }
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        clipboard::read_files()
    }

    fn link_choices(&mut self, items: Option<&[FsPath]>, dest: &FsPath) -> flit_core::provider::LinkChoices {
        match items {
            Some(items) => crate::links::choices(items, dest),
            None => clipboard::read_files().map_or_else(|_| Default::default(), |(items, _)| crate::links::choices(&items, dest)),
        }
    }

    fn clipboard_content(&mut self) -> bool {
        crate::transfer::clipboard_content()
    }

    fn clipboard_data(&mut self, dest: &FsPath) -> Option<FileOp> {
        clipboard::read_content()?.file(dest, |p| std_path(p).symlink_metadata().is_ok())
    }

    fn clear_clipboard(&mut self) {
        clipboard::clear_if_unchanged();
    }
}

pub(crate) trait Recycle {
    fn direct(&self, path: &FsPath) -> Option<FsPath>;

    fn shell(&self, path: &FsPath, job: &Job) -> Result<FsPath, PlatformError>;

    fn restore(&self, backup: &FsPath, original: &FsPath) -> bool {
        restore_checked(backup, original)
    }
}

pub(crate) struct SystemRecycle;

impl Recycle for SystemRecycle {
    fn direct(&self, path: &FsPath) -> Option<FsPath> {
        recycle::for_undo(path)
    }

    fn shell(&self, path: &FsPath, job: &Job) -> Result<FsPath, PlatformError> {
        let stamp = engine::undo_stamp(path).ok().map(|(stamp, _)| stamp);
        let op = FileOp::Trash(vec![path.clone()]);
        let flags = FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_WANTNUKEWARNING | FOF_SILENT;
        let shell = Job::new(job.ctl().clone(), Box::new(|_| {}), Box::new(|_| {}));
        let out = unsafe { operate(flags, |fo| queue(fo, &op), || cancelled(&op), &shell, path.is_ftp()) };
        if let Some(error) = out.error {
            return Err(error);
        }
        if !std_path(path).symlink_metadata().is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
            return Err(cancelled(&op));
        }
        out.items.into_iter().find_map(|(original, backup)| {
            (same_path(&original, path) && recycle::is_original(&backup, path)).then_some(backup)
        }).or_else(|| stamp.and_then(|stamp| recycle::find_recycled(path, stamp)))
            .ok_or_else(|| PlatformError::new(-1, S::ErrorsOpsUndoFailed.en()))
    }
}

pub(crate) fn checked_op(op: &FileOp, job: &Job, recycle: &dyn Recycle) -> OpOutcome {
    let failed = || PlatformError::new(-1, S::ErrorsOpsUndoFailed.en());
    let mut out = OpOutcome::default();
    match op {
        FileOp::Trash(paths) => {
            for path in paths {
                if job.cancelled() {
                    out.error = Some(cancelled(op));
                    break;
                }
                let backup = match recycle.direct(path) {
                    Some(backup) => backup,
                    None if job.cancelled() => {
                        out.error = Some(cancelled(op));
                        break;
                    }
                    None => match recycle.shell(path, job) {
                        Ok(backup) => backup,
                        Err(error) => {
                            out.error = Some(error);
                            break;
                        }
                    },
                };
                notify(SHCNE_DELETE, path, None);
                out.items.push((path.clone(), backup));
            }
            if !out.items.is_empty() {
                unsafe { SHUpdateRecycleBinIcon() };
            }
        }
        FileOp::Restore(pairs) => {
            for (recycled, original) in pairs {
                if job.cancelled() {
                    out.error = Some(cancelled(op));
                    break;
                }
                if !recycle.restore(recycled, original) {
                    out.error = Some(failed());
                    break;
                }
                notify(SHCNE_CREATE, original, None);
                out.items.push((recycled.clone(), original.clone()));
            }
            if !out.items.is_empty() {
                unsafe { SHUpdateRecycleBinIcon() };
            }
        }
        op if engine::native(op) => {
            let ran = engine::run_with_recycler(op, job, recycle);
            return OpOutcome { items: ran.items, error: ran.error.or_else(|| ran.rest.map(|_| failed())) };
        }
        _ => out.error = Some(failed()),
    }
    out
}

#[cfg(test)]
#[path = "../test/fileops.rs"]
mod tests;
