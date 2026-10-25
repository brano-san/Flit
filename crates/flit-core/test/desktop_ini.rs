use super::*;

fn utf16(s: &str) -> Vec<u8> {
    [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect()
}

#[test]
fn explorer_view_state_and_custom_icons_are_read_in_any_encoding() {
    let ini = "[.ShellClassInfo]\r\nIconResource=C:\\Windows\\System32\\imageres.dll,-113\r\n[ViewState]\r\nMode=\r\nVid=\r\nFolderType=Pictures\r\n";
    let want = DesktopIni { icon: true, kind: FolderKind::Pictures };
    assert_eq!(parse(ini.as_bytes()), want);
    assert_eq!(parse(&utf16(ini)), want);
    assert_eq!(parse(&[&[0xEF, 0xBB, 0xBF][..], ini.as_bytes()].concat()), want);
    assert_eq!(parse(b"[.shellclassinfo]\niconfile = x.ico\niconindex=0\n[viewstate]\nfoldertype = Videos"), DesktopIni { icon: true, kind: FolderKind::Videos });
}

#[test]
fn unknown_or_misplaced_keys_change_nothing() {
    assert_eq!(parse(b""), DesktopIni::default());
    assert_eq!(parse(b"FolderType=Pictures\nIconResource=x"), DesktopIni::default());
    assert_eq!(parse(b"[.ShellClassInfo]\nIconResource=\nFolderType=Music\n[Other]\nIconFile=x"), DesktopIni::default());
    assert_eq!(parse(b"[ViewState]\nFolderType=Music").kind, FolderKind::Music);
    assert_eq!(parse(b"[ViewState]\nFolderType=Documents").kind, FolderKind::Documents);
    assert_eq!(parse(b"[ViewState]\nFolderType=Generic").kind, FolderKind::Generic);
    assert_eq!(parse(b"[ViewState]\nFolderType=Contacts").kind, FolderKind::Unknown);
    assert_eq!(FolderKind::from_u8(5), FolderKind::Videos);
    assert_eq!(FolderKind::from_u8(200), FolderKind::Unknown);
}
