use super::*;

fn text(units: [u16; 260]) -> String {
    String::from_utf16_lossy(&units[..units.iter().position(|&c| c == 0).unwrap_or(units.len())])
}

fn read_description(data: &IDataObject) -> (DROPIMAGETYPE, String, String) {
    let d = unsafe {
        let mut medium = data.GetData(&description_format()).unwrap();
        let bytes = global_bytes(medium.u.hGlobal).unwrap();
        ReleaseStgMedium(&mut medium);
        assert!(bytes.len() >= size_of::<DROPDESCRIPTION>());
        std::ptr::read_unaligned(bytes.as_ptr() as *const DROPDESCRIPTION)
    };
    let (kind, message, insert) = (d.r#type, d.szMessage, d.szInsert);
    (kind, text(message), text(insert))
}

#[test]
fn drop_captions_travel_in_the_dragged_data_object() {
    crate::com_sta();
    let dir = crate::test_temp().join(format!("flit-dropdesc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let file = FsPath::parse(dir.join("a.txt").to_str().unwrap()).unwrap();
    let data: IDataObject = Pidls::parse(&[file]).unwrap().ui_object(HWND::default()).unwrap();
    describe(&data, Some(&(DROPEFFECT_MOVE.0, "Move to %1".into(), "Documents".into())));
    assert_eq!(read_description(&data), (DROPIMAGETYPE(DROPEFFECT_MOVE.0 as i32), "Move to %1".to_owned(), "Documents".to_owned()));
    describe(&data, Some(&(DROPEFFECT_COPY.0, "Copy to %1".into(), "x".repeat(400))));
    assert_eq!(read_description(&data).2.len(), 259, "long names are cut to fit");
    describe(&data, None);
    assert_eq!(read_description(&data).0, DROPIMAGE_INVALID);
    assert!(!offer_target_folder(&data, Some(&FsPath::parse(dir.to_str().unwrap()).unwrap())), "a shell drag never hands the copy over to its source");
    let _ = std::fs::remove_dir_all(&dir);
}

const SEVEN_ZIP_DO_NOT_PROCESS_IN_TARGET: u32 = 1 << 1;
const SEVEN_ZIP_DO_NOT_WAIT_FINISH: u32 = 1 << 3;
const SEVEN_ZIP_WAIT_FINISH: u32 = 1 << 4;
const SEVEN_ZIP_TEMP_FILES: u32 = 1 << 6;
const SEVEN_ZIP_NAMES_ARE_PARENT: u32 = 1 << 7;
const SEVEN_ZIP_SET_TARGET_FOLDER: u32 = 1 << 8;
const SEVEN_ZIP_SET_TARGET_FOLDER_NON_EMPTY: u32 = 1 << 9;
const SEVEN_ZIP_NEED_EXTRACT_OP_TO_FS: u32 = 1 << 10;
const SEVEN_ZIP_LEFT_BUTTON: u32 = 1 << 14;

fn registered(name: &str) -> u16 {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    unsafe { RegisterClipboardFormatW(windows_core::PCWSTR(wide.as_ptr())) as u16 }
}

fn hglobal_medium(bytes: &[u8]) -> STGMEDIUM {
    let h = unsafe { global(bytes) }.unwrap();
    STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: ManuallyDrop::new(None) }
}

fn as_fs(p: &std::path::Path) -> FsPath {
    FsPath::parse(p.to_str().unwrap()).unwrap()
}

fn as_std(p: &FsPath) -> std::path::PathBuf {
    let wide = p.to_wide(false);
    std::path::PathBuf::from(String::from_utf16_lossy(&wide[..wide.len() - 1]))
}

#[implement(IDataObject)]
struct SevenZipDrag {
    folder: std::path::PathBuf,
    temp_files: bool,
    names_are_parent: Cell<bool>,
    need_call_copy: Cell<bool>,
    copied: Cell<bool>,
    do_not_process_in_target: Cell<bool>,
    target_folder: RefCell<Option<String>>,
}

impl SevenZipDrag {
    fn extract_into(&self, dir: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("readme.txt"), "from archive").unwrap();
    }

    fn hdrop(&self) -> Vec<u8> {
        let paths = if self.names_are_parent.get() { vec![as_fs(&self.folder)] } else { vec![as_fs(&self.folder.join("readme.txt"))] };
        crate::clipboard::dropfiles(&paths)
    }

    fn transfer(&self) -> Vec<u8> {
        let mut flags = SEVEN_ZIP_LEFT_BUTTON;
        if self.temp_files {
            flags |= SEVEN_ZIP_TEMP_FILES | SEVEN_ZIP_WAIT_FINISH | SEVEN_ZIP_NEED_EXTRACT_OP_TO_FS;
        } else {
            flags |= SEVEN_ZIP_DO_NOT_WAIT_FINISH;
        }
        if self.names_are_parent.get() {
            flags |= SEVEN_ZIP_NAMES_ARE_PARENT;
        }
        if let Some(folder) = &*self.target_folder.borrow() {
            flags |= SEVEN_ZIP_SET_TARGET_FOLDER;
            if !folder.is_empty() {
                flags |= SEVEN_ZIP_SET_TARGET_FOLDER_NON_EMPTY;
            }
        }
        if self.do_not_process_in_target.get() {
            flags |= SEVEN_ZIP_DO_NOT_PROCESS_IN_TARGET;
        }
        let mut out = vec![0u8; 112];
        out[0..4].copy_from_slice(&3u32.to_le_bytes());
        out[4..8].copy_from_slice(&112u32.to_le_bytes());
        out[8..16].copy_from_slice(&1u64.to_le_bytes());
        out[64..68].copy_from_slice(&flags.to_le_bytes());
        out
    }

    fn query_continue_drag_button_released(&self) {
        self.names_are_parent.set(false);
        if self.temp_files {
            if self.target_folder.borrow().as_ref().is_some_and(|f| !f.is_empty()) {
                self.do_not_process_in_target.set(true);
            } else {
                self.need_call_copy.set(true);
            }
        }
    }

    fn after_do_drag_drop(&self) {
        if self.do_not_process_in_target.get() {
            let folder = self.target_folder.borrow().clone().unwrap();
            self.extract_into(std::path::Path::new(&folder));
        }
        if self.temp_files {
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }
}

impl windows::Win32::System::Com::IDataObject_Impl for SevenZipDrag_Impl {
    fn GetData(&self, format: *const FORMATETC) -> windows_core::Result<STGMEDIUM> {
        if self.need_call_copy.get() && !self.copied.replace(true) {
            self.extract_into(&self.folder);
        }
        let cf = unsafe { (*format).cfFormat };
        if cf == CF_HDROP.0 {
            Ok(hglobal_medium(&self.hdrop()))
        } else if cf == registered("7-Zip::GetTransfer") {
            Ok(hglobal_medium(&self.transfer()))
        } else {
            Err(windows::Win32::Foundation::DV_E_FORMATETC.into())
        }
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> windows_core::HRESULT {
        let cf = unsafe { (*format).cfFormat };
        if cf == CF_HDROP.0 || cf == registered("7-Zip::GetTransfer") {
            windows::Win32::Foundation::S_OK
        } else {
            windows::Win32::Foundation::DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> windows_core::HRESULT {
        windows::Win32::Foundation::E_NOTIMPL
    }

    fn SetData(&self, format: *const FORMATETC, medium: *const STGMEDIUM, _: windows_core::BOOL) -> windows_core::Result<()> {
        let (format, medium) = unsafe { (&*format, &*medium) };
        if format.cfFormat != registered("7-Zip::SetTargetFolder") || format.tymed != TYMED_HGLOBAL.0 as u32 || medium.tymed != TYMED_HGLOBAL.0 as u32 {
            return Err(windows::Win32::Foundation::E_NOTIMPL.into());
        }
        let bytes = unsafe { global_bytes(medium.u.hGlobal) }.unwrap_or_default();
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        *self.target_folder.borrow_mut() = Some(String::from_utf16_lossy(&units));
        Ok(())
    }

    fn EnumFormatEtc(&self, _: u32) -> windows_core::Result<windows::Win32::System::Com::IEnumFORMATETC> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<windows::Win32::System::Com::IAdviseSink>) -> windows_core::Result<u32> {
        Err(windows::Win32::Foundation::OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows_core::Result<windows::Win32::System::Com::IEnumSTATDATA> {
        Err(windows::Win32::Foundation::OLE_E_ADVISENOTSUPPORTED.into())
    }
}

struct FolderView {
    dest: FsPath,
    entered: RefCell<Vec<FsPath>>,
    queued_copies: RefCell<Vec<FsPath>>,
}

unsafe fn folder_view_call(node: *const (), ev: DragEvent) -> DropReply {
    let view = &*(node as *const FolderView);
    let accept = DropReply { effect: DROPEFFECT_COPY.0, caption: None, folder: Some(view.dest.clone()) };
    match ev {
        DragEvent::Enter { paths, .. } => {
            *view.entered.borrow_mut() = paths;
            accept
        }
        DragEvent::Over { .. } => accept,
        DragEvent::Drop { .. } => {
            view.queued_copies.borrow_mut().extend(view.entered.borrow().iter().cloned());
            accept
        }
        DragEvent::Leave => DropReply::default(),
    }
}

fn copy_tree(from: &std::path::Path, into: &std::path::Path) {
    let to = into.join(from.file_name().unwrap());
    if from.is_dir() {
        std::fs::create_dir_all(&to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            copy_tree(&e.unwrap().path(), &to);
        }
    } else if from.is_file() {
        std::fs::copy(from, &to).unwrap();
    }
}

fn drag_from_seven_zip(temp_files: bool) -> (std::path::PathBuf, DROPEFFECT) {
    crate::com_sta();
    let root = crate::test_temp().join(format!("flit-7zdrag-{}-{temp_files}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (folder, dest) = (root.join("7zE1234"), root.join("dest"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    let source = windows_core::ComObject::new(SevenZipDrag {
        folder: folder.clone(),
        temp_files,
        names_are_parent: Cell::new(temp_files),
        need_call_copy: Cell::new(false),
        copied: Cell::new(false),
        do_not_process_in_target: Cell::new(false),
        target_folder: RefCell::new(None),
    });
    if !temp_files {
        source.extract_into(&folder);
    }
    let data: IDataObject = source.to_interface();
    let view = FolderView { dest: as_fs(&dest), entered: RefCell::new(Vec::new()), queued_copies: RefCell::new(Vec::new()) };
    let target = Target::interface(HWND::default(), &view as *const FolderView as *const (), folder_view_call, None);
    let (keys, pt, allowed) = (MODIFIERKEYS_FLAGS(0), POINTL { x: 10, y: 10 }, DROPEFFECT_COPY | DROPEFFECT_MOVE);
    let mut effect = allowed;
    unsafe {
        target.DragEnter(&data, keys, pt, &mut effect).unwrap();
        effect = allowed;
        target.DragOver(keys, pt, &mut effect).unwrap();
        source.query_continue_drag_button_released();
        effect = allowed;
        target.Drop(&data, keys, pt, &mut effect).unwrap();
    }
    source.after_do_drag_drop();
    for p in view.queued_copies.borrow().iter() {
        copy_tree(&as_std(p), &dest);
    }
    (root, effect)
}

#[test]
fn seven_zip_extracts_an_archive_drag_straight_into_the_folder_under_the_cursor() {
    let (root, effect) = drag_from_seven_zip(true);
    let landed = std::fs::read_to_string(root.join("dest").join("readme.txt"));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(effect, DROPEFFECT_COPY);
    assert_eq!(landed.ok().as_deref(), Some("from archive"));
}

#[test]
fn seven_zip_drag_of_plain_files_is_still_copied_by_flit() {
    let (root, effect) = drag_from_seven_zip(false);
    let landed = std::fs::read_to_string(root.join("dest").join("readme.txt"));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(effect, DROPEFFECT_COPY);
    assert_eq!(landed.ok().as_deref(), Some("from archive"));
}
