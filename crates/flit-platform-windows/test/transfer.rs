use super::*;

fn descriptor(name: &str, flags: u32, attrs: u32, size: u64) -> Vec<u8> {
    let mut d = vec![0u8; DESCRIPTOR];
    d[0..4].copy_from_slice(&flags.to_le_bytes());
    d[36..40].copy_from_slice(&attrs.to_le_bytes());
    d[56..64].copy_from_slice(&(UNIX_EPOCH_TICKS + 10_000_000).to_le_bytes());
    d[64..68].copy_from_slice(&((size >> 32) as u32).to_le_bytes());
    d[68..72].copy_from_slice(&(size as u32).to_le_bytes());
    for (k, u) in name.encode_utf16().enumerate() {
        d[DESCRIPTOR_NAME_AT + 2 * k..DESCRIPTOR_NAME_AT + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
    }
    d
}

fn group(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = (items.len() as u32).to_le_bytes().to_vec();
    items.iter().for_each(|d| out.extend_from_slice(d));
    out
}

fn named(names: &[(&str, bool)]) -> Vec<Described> {
    names.iter().map(|&(n, dir)| Described { name: n.into(), dir, size: None, written: None }).collect()
}

#[test]
fn virtual_files_are_read_from_their_group_descriptor() {
    let bytes = group(&[descriptor("report.pdf", FD_FILESIZE | FD_WRITESTIME, 0, 5 << 32 | 7), descriptor("photos", FD_ATTRIBUTES, ATTRIBUTE_DIRECTORY, 0)]);
    let d = descriptors(&bytes);
    assert_eq!(d[0], Described { name: "report.pdf".into(), dir: false, size: Some(5 << 32 | 7), written: Some(UNIX_EPOCH_TICKS + 10_000_000) });
    assert_eq!((d[1].name.as_str(), d[1].dir, d[1].size), ("photos", true, None));
    assert_eq!(descriptors(&bytes[..bytes.len() - 1]).len(), 1, "a cut short group keeps the whole entries");
    assert!(descriptors(&[0xFF; 4]).is_empty());
}

#[test]
fn what_a_drop_or_paste_carries_decides_how_flit_saves_it() {
    let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let all = Offered { items: true, files: true, url: true };
    assert_eq!(content(Offered { url: true, ..Default::default() }, &[]), Some(Content::Url), "an address bar drag");
    assert_eq!(content(Offered { files: true, url: true, ..Default::default() }, &names(&["Example Domain.URL"])), Some(Content::Url), "a link from a page");
    assert_eq!(content(Offered { files: true, url: true, ..Default::default() }, &names(&["cat.png"])), Some(Content::Files), "an image from a page is saved, not linked");
    assert_eq!(content(Offered { files: true, ..Default::default() }, &names(&["invoice.pdf"])), Some(Content::Files), "an Outlook attachment");
    assert_eq!(content(all, &names(&["inner.txt"])), Some(Content::Items), "items inside a zip folder or on a phone go through the shell");
    assert_eq!(content(Offered { files: true, ..Default::default() }, &[]), None);
    assert_eq!(content(Offered::default(), &[]), None, "plain text is ignored");
}

#[test]
fn shell_id_lists_become_absolute_item_ids() {
    let parent = [4u8, 0, 0xAA, 0xBB, 0, 0];
    let (a, b) = ([3u8, 0, 1, 0, 0], [5u8, 0, 2, 3, 4, 0, 0]);
    let header = 4 + 4 * 3;
    let mut cida = 2u32.to_le_bytes().to_vec();
    for o in [header, header + parent.len(), header + parent.len() + a.len()] {
        cida.extend_from_slice(&(o as u32).to_le_bytes());
    }
    cida.extend_from_slice(&parent);
    cida.extend_from_slice(&a);
    cida.extend_from_slice(&b);
    assert_eq!(cida_pidls(&cida), [vec![4, 0, 0xAA, 0xBB, 3, 0, 1, 0, 0], vec![4, 0, 0xAA, 0xBB, 5, 0, 2, 3, 4, 0, 0]]);
    assert_eq!(cida_pidls(&cida[..cida.len() - 2]).len(), 1, "a broken id list stops there");
    assert!(cida_pidls(&[1, 0, 0]).is_empty());
}

#[test]
fn saved_names_stay_inside_the_folder_and_never_overwrite() {
    let items = named(&[("a.txt", false), ("docs", true), (r"docs\in.txt", false), (r"..\evil.txt", false), (r"C:\x.txt", false), ("b?.txt", false), ("a.txt", false)]);
    let plan = placement(&items, |n| n.eq_ignore_ascii_case("a.txt") || n == "docs");
    let shown: Vec<Option<String>> = plan.into_iter().map(|p| p.map(|p| p.join("\\"))).collect();
    let want = [Some("a (2).txt"), Some("docs (2)"), Some(r"docs (2)\in.txt"), None, Some(r"C_\x.txt"), Some("b_.txt"), Some("a (2).txt")];
    assert_eq!(shown, want.map(|w| w.map(str::to_owned)));
}

#[test]
fn links_become_internet_shortcuts_named_after_the_page() {
    assert_eq!(internet_shortcut("https://example.com/a b"), b"[InternetShortcut]\r\nURL=https://example.com/a b\r\n");
    assert_eq!(internet_shortcut(" https://example.com/\r\nIconFile=x "), b"[InternetShortcut]\r\nURL=https://example.com/IconFile=x\r\n", "nothing can be smuggled into the file");
    assert_eq!(internet_shortcut("https://ex.com/é"), b"[InternetShortcut]\r\nURL=https://ex.com/%C3%A9\r\n");
    assert_eq!(url_name("https://example.com/x", Some("Example: Domain.url")), "Example_ Domain.url");
    assert_eq!(url_name("https://user@www.example.com:8080/x?q", None), "www.example.com.url");
    assert_eq!(url_name("about:blank", None), "Link.url");
    assert_eq!(url_name("https://e.com", Some(&"t".repeat(300))).len(), MAX_TITLE + 4);
    let wide: Vec<u8> = "https://e.com\0junk".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!((url_text(&wide, true).as_deref(), url_text(b"http://a\0b", false).as_deref(), url_text(b"  ", false)), (Some("https://e.com"), Some("http://a"), None));
}

fn temp(tag: &str) -> std::path::PathBuf {
    let root = crate::test_temp().join(format!("flit-held-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("dest")).unwrap();
    root
}

fn fs(p: &std::path::Path) -> FsPath {
    FsPath::parse(p.to_str().unwrap()).unwrap()
}

fn on_worker(dest: FsPath, moving: bool) -> OpOutcome {
    let worker = std::thread::spawn(move || {
        crate::com_sta();
        paste_dropped(&dest, moving, &Job::detached(flit_core::provider::Resolve::Skip))
    });
    while !worker.is_finished() {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.join().unwrap()
}

fn url_data(url: &str) -> IDataObject {
    use windows::Win32::System::Com::STGMEDIUM;
    use windows::Win32::UI::Shell::SHCreateDataObject;
    let data: IDataObject = unsafe { SHCreateDataObject(None, None, None) }.unwrap();
    let wide: Vec<u8> = url.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    unsafe {
        let h = crate::clipboard::global(&wide).unwrap();
        let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
        data.SetData(&etc(format(CFSTR_INETURLW), -1, TYMED_HGLOBAL.0 as u32), &medium, true).unwrap();
    }
    data
}

#[test]
fn a_dropped_link_and_dropped_shell_items_are_saved_later_by_the_operation_on_its_own_thread() {
    crate::com_sta();
    let root = temp("later");
    let dest = fs(&root.join("dest"));
    let data = url_data("https://example.com/page");
    assert_eq!(content_of(&data), Some(Content::Url));
    let cached = read_drop_url(&data).unwrap();
    assert!(flit_core::git::dropped_repo_url(&cached).is_none());
    assert!(hold_drop(&data, Content::Url, &dest, Some(cached)).is_some());
    let out = on_worker(dest.clone(), false);
    assert!(out.error.is_none(), "{:?}", out.error);
    let [(made, _)] = out.items.try_into().unwrap();
    assert_eq!(made, dest.join(b"example.com.url"));
    assert!(std::fs::read_to_string(root.join("dest").join("example.com.url")).unwrap().contains("URL=https://example.com/page"));
    std::fs::write(root.join("photo.jpg"), "jpeg").unwrap();
    let items: IDataObject = crate::context_menu::Pidls::parse(&[fs(&root.join("photo.jpg"))]).unwrap().ui_object(windows::Win32::Foundation::HWND::default()).unwrap();
    assert!(hold_drop(&items, Content::Items, &dest, None).is_some());
    let out = on_worker(dest.clone(), false);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(std::fs::read_to_string(root.join("dest").join("photo.jpg")).unwrap(), "jpeg");
    assert!(root.join("photo.jpg").exists(), "a copy leaves the original");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_drop_flit_declined_is_released_and_never_saved() {
    crate::com_sta();
    let root = temp("declined");
    let dest = fs(&root.join("dest"));
    let ticket = hold_drop(&url_data("https://example.org/"), Content::Url, &dest, None).unwrap();
    release_drop(ticket);
    let out = on_worker(dest, false);
    assert!(out.error.is_some());
    assert_eq!(std::fs::read_dir(root.join("dest")).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repository_urls_come_from_the_native_link_format() {
    crate::com_sta();
    let data = url_data("https://github.com/owner/repo?tab=readme");
    let url = read_drop_url(&data).unwrap();
    assert_eq!(flit_core::git::dropped_repo_url(&url), Some("https://github.com/owner/repo"));
    assert!(read_drop_url(&url_data(&format!("https://example.org/{}.git", "a".repeat(MAX_REPO_URL)))).is_none());
}
