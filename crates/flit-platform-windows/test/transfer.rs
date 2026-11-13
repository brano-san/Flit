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
