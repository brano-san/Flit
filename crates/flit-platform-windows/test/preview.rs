use super::*;
use crate::com_sta;
use std::path::PathBuf;

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Dir {
    let d = crate::test_temp().join(format!("flit-preview-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Dir(d)
}

fn fs(p: &std::path::Path) -> FsPath {
    FsPath::parse(p.to_str().unwrap()).unwrap()
}

pub(crate) fn bmp(w: u32, h: u32, bgr: [u8; 3]) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend(b"BM");
    b.extend(size.to_le_bytes());
    b.extend([0u8; 4]);
    b.extend(54u32.to_le_bytes());
    b.extend(40u32.to_le_bytes());
    b.extend(w.to_le_bytes());
    b.extend(h.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(24u16.to_le_bytes());
    b.extend([0u8; 24]);
    for _ in 0..h {
        for _ in 0..w {
            b.extend(bgr);
        }
        b.extend(vec![0u8; (row - w * 3) as usize]);
    }
    b
}

#[test]
fn decodes_and_scales_images_into_premultiplied_rgba() {
    com_sta();
    let d = temp("wic");
    let p = d.0.join("red.bmp");
    std::fs::write(&p, bmp(40, 20, [0, 0, 255])).unwrap();
    let b = decode_image(&fs(&p), 10).unwrap();
    assert_eq!((b.width, b.height), (10, 5));
    assert_eq!(&b.rgba[..4], &[255, 0, 0, 255]);
    let full = decode_image(&fs(&p), 1600).unwrap();
    assert_eq!((full.width, full.height), (40, 20));
    std::fs::write(d.0.join("broken.png"), b"not a png").unwrap();
    assert!(decode_image(&fs(&d.0.join("broken.png")), 100).is_none());
}

#[test]
fn large_jpegs_shrink_in_the_decoder_and_keep_their_layout() {
    let d = temp("wic-jpeg");
    let p = d.0.join("gradient.jpg");
    crate::shell::tests::write_jpeg(&p, 802, 601, 0);
    let b = decode_image(&fs(&p), 100).unwrap();
    assert_eq!((b.width, b.height), (100, 75));
    let green = |y: u32| (0..b.width).map(|x| b.rgba[((y * b.width + x) * 4 + 1) as usize] as u32).sum::<u32>() / b.width;
    assert!(green(0) < 40 && green(b.height - 1) > 215, "top {} bottom {}", green(0), green(b.height - 1));
    assert!(b.rgba.chunks_exact(4).all(|px| px[3] == 255));
}

#[test]
fn reads_grouped_properties_and_finds_no_handler_for_unknown_types() {
    com_sta();
    let d = temp("props");
    let p = d.0.join("notes.txt");
    std::fs::write(&p, "hello").unwrap();
    let groups = properties(&fs(&p));
    assert!(groups.iter().all(|g| !g.rows.is_empty()));
    assert!(groups.iter().flat_map(|g| &g.rows).any(|(k, v)| !k.is_empty() && !v.is_empty()));
    assert_eq!(handler(b"flit-no-such-type"), None);
}

#[test]
fn a_hard_linked_file_shows_its_link_group_first_and_a_plain_file_shows_none() {
    com_sta();
    let d = temp("link-props");
    let p = d.0.join("data.txt");
    std::fs::write(&p, "hello").unwrap();
    let link = S::PropertiesLink.t();
    assert!(properties(&fs(&p)).iter().all(|g| g.title != link));
    std::fs::hard_link(&p, d.0.join("again.txt")).unwrap();
    let groups = properties(&fs(&p));
    assert_eq!(groups[0].title, link);
    assert_eq!(groups[0].rows, [(S::PropertiesLinkType.t().to_owned(), S::PropertiesHardLink.t().to_owned()), (S::PropertiesHardLinks.t().to_owned(), "2".to_owned())]);
    assert!(groups.len() > 1, "the shell's own groups still follow");
}

#[test]
fn a_junction_lists_its_target_and_a_link_without_a_readable_target_lists_only_its_kind() {
    let target = FsPath::parse(r"C:\Data\Photos").unwrap();
    let group = link_group(&LinkInfo::Junction(Some(target)));
    assert_eq!(group.rows, [(S::PropertiesLinkType.t().to_owned(), S::PropertiesJunction.t().to_owned()), (S::PropertiesLinkTarget.t().to_owned(), r"C:\Data\Photos".to_owned())]);
    assert_eq!(link_group(&LinkInfo::Symbolic(None)).rows, [(S::PropertiesLinkType.t().to_owned(), S::PropertiesSymbolicLink.t().to_owned())]);
}

#[test]
fn hands_the_file_to_a_running_quick_look_over_its_pipe() {
    use std::io::Read;
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};
    let name = format!(r"\\.\pipe\flit-test-quicklook-{}", std::process::id());
    assert!(!quick_look_running(&name, r"C:\a.txt"));
    let wide = wide_str(&name);
    let h = unsafe { CreateNamedPipeW(PCWSTR(wide.as_ptr()), PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, None) };
    assert!(!h.is_invalid());
    assert!(quick_look_running(&name, r"C:\папка\a b.txt"));
    let _ = unsafe { ConnectNamedPipe(h, None) };
    let mut got = String::new();
    unsafe { std::fs::File::from_raw_handle(h.0) }.read_to_string(&mut got).unwrap();
    assert_eq!(got, "QuickLook.App.PipeMessages.Toggle|C:\\папка\\a b.txt\r\n");
    assert!(quick_look_pipe().starts_with(r"\\.\pipe\QuickLook.App.Pipe.S-1-5-"));
}

static SEER_GOT: std::sync::Mutex<Option<(usize, String)>> = std::sync::Mutex::new(None);

unsafe extern "system" fn fake_seer(hwnd: windows::Win32::Foundation::HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
    if msg == WM_COPYDATA {
        let data = &*(lp.0 as *const COPYDATASTRUCT);
        let text = std::slice::from_raw_parts(data.lpData as *const u16, data.cbData as usize / 2);
        *SEER_GOT.lock().unwrap() = Some((data.dwData, String::from_utf16_lossy(text)));
        return windows::Win32::Foundation::LRESULT(1);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[test]
fn hands_the_file_to_a_seer_window_by_copy_data() {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, RegisterClassExW, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP};
    let class = format!("FlitFakeSeer{}", std::process::id());
    assert!(!seer(&class, r"C:\a.txt"));
    let wclass = wide_str(&class);
    unsafe {
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(fake_seer), lpszClassName: PCWSTR(wclass.as_ptr()), ..Default::default() };
        assert_ne!(RegisterClassExW(&wc), 0);
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(wclass.as_ptr()), PCWSTR::null(), WS_POPUP, 0, 0, 0, 0, None, None, None, None).unwrap();
        assert!(seer(&class, r"C:\папка\a.txt"));
        let _ = DestroyWindow(hwnd);
    }
    let (kind, text) = SEER_GOT.lock().unwrap().take().unwrap();
    assert_eq!(kind, SEER_INVOKE);
    assert_eq!(text, "C:\\папка\\a.txt\0");
}

#[test]
fn a_download_from_the_internet_is_blocked_and_an_intranet_or_plain_file_is_not() {
    let d = temp("zone");
    let mark = |name: &str, zone: Option<u32>| {
        let p = d.0.join(name);
        std::fs::write(&p, "x").unwrap();
        if let Some(z) = zone {
            std::fs::write(format!("{}:Zone.Identifier", p.display()), format!("[ZoneTransfer]\r\nZoneId={z}\r\n")).unwrap();
        }
        fs(&p)
    };
    assert!(blocked(&mark("web.exe", Some(3))));
    assert!(blocked(&mark("restricted.exe", Some(4))));
    assert!(!blocked(&mark("intranet.exe", Some(1))));
    assert!(!blocked(&mark("local.exe", None)));
    assert!(!blocked(&fs(&d.0)));
    assert_eq!(properties(&mark("zip.exe", Some(3))).last(), Some(&security_group()));
}

#[test]
fn column_properties_give_picture_dimensions_and_program_versions_and_stay_empty_otherwise() {
    com_sta();
    let d = temp("columns");
    let pic = d.0.join("pic.bmp");
    std::fs::write(&pic, bmp(7, 5, [10, 20, 30])).unwrap();
    let got = column_props(&fs(&pic));
    assert_eq!(got.len(), 4);
    assert!(got[0].contains('7') && got[0].contains('5'), "{got:?}");
    assert!(got[2].is_empty() && got[3].is_empty(), "{got:?}");
    let exe = column_props(&FsPath::parse(r"C:\Windows\notepad.exe").unwrap());
    assert!(exe[3].chars().next().is_some_and(|c| c.is_ascii_digit()), "{exe:?}");
    assert!(column_props(&fs(&d.0.join("missing.bmp"))).is_empty());
}
