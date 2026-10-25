use super::*;

fn encode(text: &str, e: &'static encoding_rs::Encoding) -> Vec<u8> {
    e.encode(text).0.into_owned()
}

fn cp437_encode(text: &str) -> Vec<u8> {
    text.chars().map(|c| if c.is_ascii() { c as u8 } else { 0x80 + CP437_HIGH.iter().position(|&h| h == c).unwrap() as u8 }).collect()
}

fn guess(names: &[Vec<u8>]) -> Option<Encoding> {
    detect(names.iter().map(Vec::as_slice))
}

const RUSSIAN: [&str; 4] = ["Отчёт за квартал.doc", "Фотографии/лето на даче.jpg", "Документы/Договор аренды.pdf", "readme.txt"];

#[test]
fn legacy_cyrillic_zips_are_recognized() {
    let dos: Vec<Vec<u8>> = RUSSIAN.iter().map(|n| encode(n, encoding_rs::IBM866)).collect();
    assert_eq!(guess(&dos), Some(Encoding::Cp866));
    assert_eq!(decode(&dos[1], Encoding::Cp866), RUSSIAN[1]);
    let ansi: Vec<Vec<u8>> = RUSSIAN.iter().map(|n| encode(n, encoding_rs::WINDOWS_1251)).collect();
    assert_eq!(guess(&ansi), Some(Encoding::Cp1251));
    let koi: Vec<Vec<u8>> = RUSSIAN.iter().map(|n| encode(n, encoding_rs::KOI8_R)).collect();
    assert_eq!(guess(&koi), Some(Encoding::Koi8R));
    assert_eq!(guess(&[encode("Привет", encoding_rs::IBM866)]), Some(Encoding::Cp866));
    assert_eq!(guess(&[encode("Привет", encoding_rs::WINDOWS_1251)]), Some(Encoding::Cp1251));
}

#[test]
fn western_and_asian_names_are_recognized() {
    let german = [cp437_encode("Grüße aus Köln.txt"), cp437_encode("Café/Menü.pdf")];
    assert_eq!(guess(&german), Some(Encoding::Cp437));
    assert_eq!(decode(&german[0], Encoding::Cp437), "Grüße aus Köln.txt");
    let ansi = [encode("Café crème.txt", encoding_rs::WINDOWS_1252), encode("Señor.doc", encoding_rs::WINDOWS_1252)];
    assert_eq!(guess(&ansi), Some(Encoding::Cp1252));
    let japanese = [encode("写真/夏休みの思い出.jpg", encoding_rs::SHIFT_JIS), encode("ドキュメント.txt", encoding_rs::SHIFT_JIS)];
    assert_eq!(guess(&japanese), Some(Encoding::ShiftJis));
    assert_eq!(guess(&["Отчёт.txt".as_bytes().to_vec()]), Some(Encoding::Utf8));
    assert_eq!(guess(&[b"plain.txt".to_vec()]), None);
}

#[test]
fn unsafe_entry_names_are_rejected() {
    for bad in ["../evil.txt", "a/../../evil", "/etc/passwd", "\\\\server\\share\\x", "C:/Windows/x.dll", "c:x", "file.txt:stream", "CON", "dir/nul.txt", "COM1.log", "LPT9", "COM¹", "aux .txt", "trailing.", "space ", "a/b./c", "ctl\u{1}x", "q?.txt", "", "./", "a/<b>"] {
        assert_eq!(safe_path_for(bad, true), None, "{bad:?}");
    }
    for windows in [true, false] {
        assert_eq!(safe_path_for("dir/sub/file.txt", windows).as_deref(), Some("dir/sub/file.txt"));
        assert_eq!(safe_path_for("./dir//file.txt", windows).as_deref(), Some("dir/file.txt"));
        assert_eq!(safe_path_for("dir\\file.txt", windows).as_deref(), Some("dir/file.txt"));
        assert_eq!(safe_path_for("console.txt", windows).as_deref(), Some("console.txt"));
        assert_eq!(safe_path_for("COM10", windows).as_deref(), Some("COM10"));
        assert_eq!(safe_path_for(".hidden", windows).as_deref(), Some(".hidden"));
    }
    assert_eq!(safe_path("dir/file.txt"), safe_path_for("dir/file.txt", cfg!(windows)));
}

#[test]
fn unix_extraction_allows_names_windows_forbids() {
    for fine in ["file.txt:stream", "CON", "dir/nul.txt", "aux .txt", "trailing.", "space ", "q?.txt", "a/<b>", "c:x"] {
        assert_eq!(safe_path_for(fine, false).as_deref(), Some(fine), "{fine:?}");
    }
    for bad in ["../evil.txt", "a/../../evil", "/etc/passwd", "\\\\server\\share\\x", "ctl\u{1}x", "", "./", &"x".repeat(256)] {
        assert_eq!(safe_path_for(bad, false), None, "{bad:?}");
    }
}
