use super::*;
use windows::core::w;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn dropfiles_round_trip_wide_and_ansi() {
    let paths = vec![p(r"C:\a b\файл.txt"), p(r"\\srv\share\😀"), p(r"D:\x")];
    assert_eq!(parse_dropfiles(&dropfiles(&paths)), paths);
    let mut ansi = vec![20, 0, 0, 0];
    ansi.extend_from_slice(&[0; 16]);
    ansi.extend_from_slice(b"C:\\one\0C:\\two\0\0");
    assert_eq!(parse_dropfiles(&ansi), vec![p(r"C:\one"), p(r"C:\two")]);
    assert!(parse_dropfiles(&[1, 2, 3]).is_empty());
    assert!(parse_dropfiles(&dropfiles(&[])).is_empty());
}

#[test]
fn preferred_drop_effect_means_move_only_for_a_pure_move() {
    assert!(effect_is_move(DROPEFFECT_MOVE.0));
    assert!(!effect_is_move(DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0));
    assert!(!effect_is_move(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0));
    assert!(!effect_is_move(0));
}

unsafe fn saved_text() -> Option<Vec<u8>> {
    open(None).ok()?;
    let text = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|h| global_bytes(HGLOBAL(h.0)));
    let _ = CloseClipboard();
    text
}

unsafe fn restore_text(owner: HWND, text: Option<Vec<u8>>) {
    if OpenClipboard(Some(owner)).is_ok() {
        let _ = EmptyClipboard();
        if let Some(t) = text {
            put(CF_UNICODETEXT.0 as u32, &t);
        }
        let _ = CloseClipboard();
    }
}

#[test]
#[ignore]
fn files_written_to_the_clipboard_read_back_with_their_effect() {
    unsafe {
        let owner = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), None, WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None).unwrap();
        let before = saved_text();
        let paths = vec![p(r"C:\flit\a.txt"), p(r"C:\flit\b c")];
        let cut = write_files(owner, &paths, true).then(read_files);
        let copy = write_files(owner, &paths, false).then(read_files);
        clear_if_unchanged();
        let cleared = read_files();
        restore_text(owner, before);
        let _ = DestroyWindow(owner);
        assert_eq!(cut.unwrap().unwrap(), (paths.clone(), true));
        assert_eq!(copy.unwrap().unwrap(), (paths, false));
        assert!(cleared.is_err());
    }
}
