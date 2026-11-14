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

fn picture() -> Bitmap {
    Bitmap { width: 2, height: 2, rgba: vec![255, 0, 0, 255, 0, 64, 0, 128, 0, 0, 0, 0, 10, 20, 30, 255] }
}

#[test]
fn images_become_bottom_up_dibs_with_straight_alpha_or_on_white() {
    let v5 = dib(&picture(), true);
    let word = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    assert_eq!(v5.len(), 124 + 16);
    assert_eq!((word(&v5, 0), word(&v5, 4), word(&v5, 8), word(&v5, 16)), (124, 2, 2, 3));
    assert_eq!((word(&v5, 52), word(&v5, 56), word(&v5, 108)), (0xFF00_0000, 0x7352_4742, 4));
    assert_eq!(&v5[124..140], &[0, 0, 0, 0, 30, 20, 10, 255, 0, 0, 255, 255, 0, 128, 0, 128], "rows run bottom up in BGRA");
    let plain = dib(&picture(), false);
    assert_eq!((plain.len(), word(&plain, 0), word(&plain, 16)), (40 + 16, 40, 0));
    assert_eq!(&plain[40..56], &[255, 255, 255, 255, 30, 20, 10, 255, 0, 0, 255, 255, 127, 191, 127, 255], "transparency turns white");
    assert!(dib(&Bitmap { width: 3, height: 3, rgba: vec![0; 8] }, true).is_empty());
    assert!(dib(&Bitmap { width: 0, height: 0, rgba: Vec::new() }, false).is_empty());
}

#[test]
fn images_encode_to_png_that_decodes_back_to_the_same_pixels() {
    crate::com_sta();
    let bytes = png(&picture()).expect("png");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    let dir = crate::test_temp().join(format!("flit-clip-png-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("copy.png");
    std::fs::write(&file, &bytes).unwrap();
    let back = crate::preview::decode_image(&p(file.to_str().unwrap()), 64);
    let _ = std::fs::remove_dir_all(&dir);
    let back = back.expect("decodes");
    assert_eq!((back.width, back.height), (2, 2));
    assert_eq!(back.rgba, picture().rgba);
    assert!(png(&Bitmap { width: 4, height: 4, rgba: vec![1, 2, 3] }).is_none());
}

#[test]
#[ignore]
fn images_written_to_the_clipboard_offer_png_and_dibs() {
    unsafe {
        crate::com_sta();
        let owner = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), None, WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None).unwrap();
        let before = saved_text();
        let written = write_image(owner, &picture());
        let formats = open(None).is_ok().then(|| {
            let got = [png_format(), CF_DIBV5.0 as u32, CF_DIB.0 as u32].map(|f| GetClipboardData(f).ok().and_then(|h| global_bytes(HGLOBAL(h.0))).map_or(0, |b| b.len()));
            let _ = CloseClipboard();
            got
        });
        restore_text(owner, before);
        let _ = DestroyWindow(owner);
        assert!(written);
        let [png, v5, plain] = formats.unwrap();
        assert!(png > 8 && v5 >= 140 && plain >= 56);
    }
}

#[test]
fn copied_pictures_read_back_from_dibs_of_any_row_order() {
    let pic = picture();
    let opaque = |b: &Bitmap| b.rgba.chunks_exact(4).all(|p| p[3] == 255);
    let v5 = from_dib(&dib(&pic, true)).unwrap();
    assert_eq!((v5.width, v5.height), (2, 2));
    assert_eq!(&v5.rgba[..4], &pic.rgba[..4], "rows come back top first");
    assert_eq!(v5.rgba[12..], pic.rgba[12..]);
    assert_eq!(v5.rgba[7], 128, "transparency survives");
    let plain = from_dib(&dib(&pic, false)).unwrap();
    assert!(opaque(&plain) && plain.rgba[..4] == [255, 0, 0, 255]);
    let mut top_down = dib(&pic, false);
    top_down[8..12].copy_from_slice(&(-2i32).to_le_bytes());
    assert_eq!(from_dib(&top_down).unwrap().rgba[..4], plain.rgba[8..12], "a negative height means the first row is the top");
    let mut rgb = vec![0u8; 40];
    rgb[0] = 40;
    rgb[4] = 1;
    rgb[8] = 1;
    rgb[12] = 1;
    rgb[14] = 24;
    rgb.extend_from_slice(&[30, 20, 10, 0]);
    assert_eq!(from_dib(&rgb).unwrap().rgba, [10, 20, 30, 255], "24-bit rows are padded to four bytes");
}

#[test]
fn broken_or_unsupported_dibs_are_refused() {
    let mut eight = dib(&picture(), false);
    eight[14] = 8;
    assert!(from_dib(&eight).is_none());
    let whole = dib(&picture(), false);
    assert!(from_dib(&whole[..whole.len() - 1]).is_none(), "missing pixels");
    assert!(from_dib(&[]).is_none() && from_dib(&whole[..20]).is_none());
}

#[test]
fn phone_items_go_on_the_clipboard_as_shell_items_and_disk_files_as_plain_paths() {
    let phone = FsPath::device(b"Pixel 7").unwrap().join(b"Internal storage").join(b"DCIM").join(b"a.jpg");
    let disk = FsPath::parse(r"C:\Users\x\a.jpg").unwrap();
    assert!(needs_items(std::slice::from_ref(&phone)));
    assert!(needs_items(&[disk.clone(), phone]));
    assert!(!needs_items(&[disk]));
    assert!(!needs_items(&[FsPath::parse(r"\\server\share\a.jpg").unwrap()]));
}
