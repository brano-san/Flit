use super::*;

#[test]
fn redirected_picker_paths_are_utf8_and_have_one_line_per_path() {
    let paths = [FsPath::parse("C:\\Пример😀.png").unwrap(), FsPath::parse("C:\\ещё.txt").unwrap()];
    let mut bytes = Vec::new();
    write_redirected_paths(&paths, &mut bytes).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), "C:\\Пример😀.png\nC:\\ещё.txt\n");
    bytes.clear();
    write_redirected_paths(&[], &mut bytes).unwrap();
    assert!(bytes.is_empty());
}

#[cfg(windows)]
#[test]
fn console_picker_paths_keep_unicode_and_handle_partial_writes() {
    let path = "C:\\Пример😀.png";
    let mut out = Vec::new();
    write_console_paths(&[FsPath::parse(path).unwrap()], |units| {
        let count = units.len().min(2);
        out.extend_from_slice(&units[..count]);
        Ok(count)
    })
    .unwrap();
    assert_eq!(out, format!("{path}\r\n").encode_utf16().collect::<Vec<_>>());
    assert_eq!(write_console_paths(&[FsPath::parse(path).unwrap()], |_| Ok(0)).unwrap_err().kind(), io::ErrorKind::WriteZero);
}
