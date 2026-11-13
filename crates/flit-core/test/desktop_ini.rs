use super::*;

fn utf16(s: &str) -> Vec<u8> {
    [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect()
}

#[test]
fn explorer_view_state_and_custom_icons_are_read_in_any_encoding() {
    let ini = "[.ShellClassInfo]\r\nIconResource=C:\\Windows\\System32\\imageres.dll,-113\r\n[ViewState]\r\nMode=\r\nVid=\r\nFolderType=Pictures\r\n";
    let want = DesktopIni { icon: true, kind: FolderKind::Pictures, name: None };
    assert_eq!(parse(ini.as_bytes()), want);
    assert_eq!(parse(&utf16(ini)), want);
    assert_eq!(parse(&[&[0xEF, 0xBB, 0xBF][..], ini.as_bytes()].concat()), want);
    assert_eq!(parse(b"[.shellclassinfo]\niconfile = x.ico\niconindex=0\n[viewstate]\nfoldertype = Videos"), DesktopIni { icon: true, kind: FolderKind::Videos, name: None });
}

#[test]
fn unknown_or_misplaced_keys_change_nothing() {
    assert_eq!(parse(b""), DesktopIni::default());
    assert_eq!(parse(b"FolderType=Pictures\nIconResource=x\nLocalizedResourceName=x"), DesktopIni::default());
    assert_eq!(parse(b"[.ShellClassInfo]\nIconResource=\nFolderType=Music\nLocalizedResourceName=\n[Other]\nIconFile=x\nLocalizedResourceName=x"), DesktopIni::default());
    assert_eq!(parse(b"[ViewState]\nFolderType=Music").kind, FolderKind::Music);
    assert_eq!(parse(b"[ViewState]\nFolderType=Documents").kind, FolderKind::Documents);
    assert_eq!(parse(b"[ViewState]\nFolderType=Generic").kind, FolderKind::Generic);
    assert_eq!(parse(b"[ViewState]\nFolderType=Contacts").kind, FolderKind::Unknown);
    assert_eq!(FolderKind::from_u8(5), FolderKind::Videos);
    assert_eq!(FolderKind::from_u8(200), FolderKind::Unknown);
}

#[test]
fn localized_resource_names_are_kept_raw_for_the_platform_to_resolve() {
    let documents = "\r\n[.ShellClassInfo]\r\nLocalizedResourceName=@%SystemRoot%\\system32\\shell32.dll,-21770\r\nIconResource=%SystemRoot%\\system32\\imageres.dll,-112\r\nIconFile=%SystemRoot%\\system32\\shell32.dll\r\nIconIndex=-235\r\n";
    let ini = parse(&utf16(documents));
    assert_eq!(ini.name.as_deref(), Some(r"@%SystemRoot%\system32\shell32.dll,-21770"));
    assert!(ini.icon);
    assert_eq!(parse("[.ShellClassInfo]\nlocalizedresourcename = \"Фотографии\"\n".as_bytes()).name.as_deref(), Some("Фотографии"));
}

#[test]
fn folder_labels_are_remembered_per_windows_path_regardless_of_case() {
    let docs = FsPath::parse(r"C:\Users\flit-label-test\Documents").unwrap();
    let plain = FsPath::parse(r"C:\Users\flit-label-test\Projects").unwrap();
    assert!(!known(&docs) && label(&docs).is_none());
    remember(&docs, Some("Документы"));
    remember(&plain, None);
    assert_eq!(label(&FsPath::parse(r"c:\users\FLIT-LABEL-TEST\documents").unwrap()).as_deref(), Some("Документы"));
    assert!(known(&plain) && label(&plain).is_none());
    remember(&docs, Some(""));
    assert!(known(&docs) && label(&docs).is_none(), "a removed name is forgotten");
    let unix = FsPath::parse("/home/flit-label-test/Documents").unwrap();
    remember(&unix, Some("Документы"));
    assert!(label(&unix).is_none() && known(&unix), "only Windows drive paths carry desktop.ini names");
}
