use super::*;

fn texts(d: &Doc) -> Vec<String> {
    d.paras.iter().map(|p| d.runs[p.runs.start as usize..p.runs.end as usize].iter().map(|r| d.run_text(r)).collect()).collect()
}

#[test]
fn kinds_follow_the_extension() {
    assert_eq!(kind(b"JPG"), Kind::Image);
    assert_eq!(kind(b"mp3"), Kind::Audio);
    assert_eq!(kind(b"mkv"), Kind::Video);
    assert_eq!(kind(b"log"), Kind::Text);
    assert_eq!(kind(b"md"), Kind::Markdown);
    assert_eq!(kind(b"rtf"), Kind::Rtf);
    assert_eq!(kind(b"rs"), Kind::Code(syntax::by_ext("rs").unwrap()));
    assert_eq!(kind(b"docx"), Kind::Other);
    assert_eq!(kind(b""), Kind::Other);
}

#[test]
fn decodes_boms_and_cut_utf8() {
    assert_eq!(decode(b"\xEF\xBB\xBFhi"), "hi");
    assert_eq!(decode(&[0xFF, 0xFE, b'h', 0, 0x3F, 0x04]), "hп");
    assert_eq!(decode(&[0xFE, 0xFF, 0, b'h']), "h");
    let s = "привет".as_bytes();
    assert_eq!(decode(&s[..s.len() - 1]), "приве");
    assert!(decode(b"a\xFFb").contains('\u{FFFD}'));
}

#[test]
fn binary_sniffing() {
    assert!(!looks_binary(b"plain text\r\n\tindent"));
    assert!(looks_binary(b"MZ\x90\0\x03\0\0\0"));
    assert!(looks_binary(&[1u8; 100]));
    assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]));
}

#[test]
fn copied_images_get_straight_alpha_back_from_premultiplied_pixels() {
    let premultiplied = [255, 10, 0, 255, 64, 32, 0, 128, 9, 9, 9, 0];
    assert_eq!(straight_rgba(&premultiplied), [255, 10, 0, 255, 128, 64, 0, 128, 0, 0, 0, 0]);
    assert_eq!(straight_rgba(&[200, 0, 0, 100, 1]), [255, 0, 0, 100], "a too bright channel and a torn pixel stay in range");
    assert!(straight_rgba(&[]).is_empty());
}

#[test]
fn text_docs_are_capped_and_expand_tabs() {
    let d = text_doc("a\tb\r\nsecond", None);
    assert_eq!(texts(&d), ["a   b", "second"]);
    assert!(d.wrap && d.mono && !d.truncated);
    let long = "x".repeat(TEXT_CHARS + 50);
    let d = text_doc(&long, None);
    assert!(d.truncated);
    assert_eq!(d.text.chars().count(), TEXT_CHARS);
}

#[test]
fn code_docs_keep_highlighting_across_lines() {
    let k = syntax::by_ext("rs").unwrap();
    let d = text_doc("/* a\nb */ fn x() {}", Some(k));
    assert!(!d.wrap);
    let second: Vec<(String, Tone)> = d.runs[d.paras[1].runs.start as usize..d.paras[1].runs.end as usize].iter().map(|r| (d.run_text(r).to_owned(), r.tone)).collect();
    assert_eq!(second[0], ("b */".into(), Tone::Comment));
    assert!(second.contains(&("fn".into(), Tone::Keyword)));
}

#[test]
fn rtf_keeps_text_and_drops_tables() {
    let src = [&br"{\rtf1\ansi\ansicpg1252{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\*\generator Riched20;}\pard Hello \b bold\b0\par Caf\'e9 \"[..], b"u1087?\\", b"u1088?\\tab end{\\pict 0102}\\par}"].concat();
    assert_eq!(rtf_text(&src), "Hello bold\nCafé пр\tend\n");
}

#[test]
fn rtf_handles_surrogates_and_binary() {
    let src = br"{\rtf1\uc1\u-10179?\u-8704? x\bin3 abcY}";
    assert_eq!(rtf_text(src), "\u{1F600} xY");
}

#[test]
fn folder_summary_counts_types() {
    let mut s = FolderSummary::default();
    for (n, dir, size) in [("a.TXT", false, 10), ("b.txt", false, 5), ("c.png", false, 1), ("d", true, 0), ("e", false, 2)] {
        s.add(n.as_bytes(), dir, false, size, 7);
    }
    let s = s.finish(true);
    assert_eq!((s.dirs, s.files, s.bytes, s.newest), (1, 4, 18, 7));
    assert_eq!(s.types[0], ("txt".into(), 2));
    assert_eq!(s.types.len(), 3);
}

#[test]
fn a_video_frame_waits_alone_and_a_newer_one_replaces_it() {
    let frames = Frames::default();
    let shared = frames.clone();
    let px = |w: u32| Bitmap { width: w, height: 1, rgba: vec![0; w as usize * 4] };
    assert!(frames.put(3, px(1)), "the first frame asks for a wake-up");
    assert!(!shared.put(3, px(2)), "a second frame before the first is shown asks for none");
    assert_eq!(frames.take().map(|(g, b)| (g, b.width)), Some((3, 2)));
    assert!(frames.take().is_none());
    assert!(frames.put(4, px(5)), "after the screen took a frame the next one wakes it again");
    assert_eq!(frames, shared);
    assert_ne!(frames, Frames::default());
}
