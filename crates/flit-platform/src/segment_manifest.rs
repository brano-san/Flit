pub(crate) struct Manifest {
    pub(crate) file: String,
    pub(crate) len: u64,
    pub(crate) checksum: u32,
}

impl Manifest {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut file = None;
        let mut wide_file = None;
        let mut len = None;
        let mut checksum = None;
        for (key, value) in text.trim_start_matches('\u{feff}').lines().filter_map(|line| line.split_once('=')) {
            match key.trim().to_ascii_lowercase().as_str() {
                "filename" if file.is_none() => file = Some(value),
                "filenameutf8" if wide_file.is_none() => wide_file = Some(value.trim_start_matches('\u{feff}')),
                "size" if len.is_none() => len = Some(value.trim().parse().ok()?),
                "crc32" if checksum.is_none() => {
                    let value = value.trim().trim_start_matches('$');
                    if value.len() != 8 { return None; }
                    checksum = Some(u32::from_str_radix(value, 16).ok()?);
                }
                "filename" | "filenameutf8" | "size" | "crc32" => return None,
                _ => {}
            }
        }
        let file = wide_file.or(file)?;
        if !valid_name(file) { return None; }
        Some(Self { file: file.to_owned(), len: len?, checksum: checksum? })
    }

    pub(crate) fn render(&self) -> String {
        let bom = if self.file.is_ascii() { "" } else { "\u{feff}" };
        format!("{bom}filename={}\r\nsize={}\r\ncrc32={:08X}\r\n", self.file, self.len, self.checksum)
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.chars().any(char::is_control) && !name.contains(['\\', ':'])
        && flit_core::ui::validate_name(name, cfg!(windows)).is_ok_and(|safe| safe == name)
}

pub(crate) fn decode_manifest<'a>(raw: &'a [u8], decode_ansi: impl FnOnce(&[u8]) -> Option<String>) -> std::borrow::Cow<'a, str> {
    const CP1252: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match std::str::from_utf8(raw) {
        Ok(text) => text.into(),
        Err(_) => decode_ansi(raw).unwrap_or_else(|| raw.iter().map(|&b| match b {
            0x80..=0x9f => CP1252[usize::from(b - 0x80)],
            _ => char::from(b),
        }).collect()).into(),
    }
}
