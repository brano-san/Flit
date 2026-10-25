use flit_core::archive::Encoding;

const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{A0}',
];

const GUESSES: [Encoding; 8] = [Encoding::Cp866, Encoding::Cp1251, Encoding::Cp437, Encoding::Cp1252, Encoding::Koi8R, Encoding::Cp1250, Encoding::ShiftJis, Encoding::Gbk];

fn table(e: Encoding) -> Option<&'static encoding_rs::Encoding> {
    Some(match e {
        Encoding::Utf8 => encoding_rs::UTF_8,
        Encoding::Cp437 => return None,
        Encoding::Cp866 => encoding_rs::IBM866,
        Encoding::Cp1251 => encoding_rs::WINDOWS_1251,
        Encoding::Cp1252 => encoding_rs::WINDOWS_1252,
        Encoding::Cp1250 => encoding_rs::WINDOWS_1250,
        Encoding::Koi8R => encoding_rs::KOI8_R,
        Encoding::ShiftJis => encoding_rs::SHIFT_JIS,
        Encoding::Gbk => encoding_rs::GBK,
        Encoding::Big5 => encoding_rs::BIG5,
        Encoding::EucKr => encoding_rs::EUC_KR,
    })
}

pub fn cp437(raw: &[u8]) -> String {
    raw.iter().map(|&b| if b < 0x80 { b as char } else { CP437_HIGH[b as usize - 0x80] }).collect()
}

pub fn decode(raw: &[u8], e: Encoding) -> String {
    match table(e) {
        Some(t) => t.decode_without_bom_handling(raw).0.into_owned(),
        None => cp437(raw),
    }
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

fn weight(c: char) -> i32 {
    match c {
        '\u{0430}'..='\u{044F}' => 3,
        '\u{0410}'..='\u{042F}' => 1,
        'ё' | 'Ё' | 'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ' => 0,
        c if is_cyrillic(c) => -2,
        '×' | '÷' => -5,
        'ß' | '\u{00C0}'..='\u{00FF}' => 2,
        '\u{0100}'..='\u{017F}' => 1,
        '\u{3040}'..='\u{30FF}' => 2,
        '\u{4E00}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7A3}' => 1,
        '\u{FFFD}' => -10,
        c if c.is_control() => -10,
        c if c.is_ascii() => 0,
        _ => -5,
    }
}

fn score(text: &str) -> i32 {
    let mut total = 0;
    for word in text.split(|c: char| !c.is_alphabetic()) {
        let mixed = word.chars().any(|c| c.is_ascii_alphabetic()) && word.chars().any(is_cyrillic);
        total += word.chars().map(|c| if mixed && is_cyrillic(c) { -4 } else { weight(c) }).sum::<i32>();
    }
    total + text.chars().filter(|c| !c.is_alphabetic()).map(weight).sum::<i32>()
}

pub fn detect<'a>(names: impl Iterator<Item = &'a [u8]> + Clone) -> Option<Encoding> {
    let mut legacy = names.filter(|n| !n.is_ascii()).peekable();
    legacy.peek()?;
    if legacy.clone().all(|n| std::str::from_utf8(n).is_ok()) {
        return Some(Encoding::Utf8);
    }
    let sample: Vec<&[u8]> = legacy.take(2000).collect();
    let mut best = (i32::MIN, Encoding::Cp437);
    for e in GUESSES {
        let s: i32 = sample.iter().map(|n| score(&decode(n, e))).sum();
        if s > best.0 {
            best = (s, e);
        }
    }
    Some(best.1)
}

pub fn fold(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut low = c.to_lowercase();
            match (low.next(), low.next()) {
                (Some(l), None) if l.len_utf8() == c.len_utf8() => l,
                _ => c,
            }
        })
        .collect()
}

fn reserved(c: &str) -> bool {
    let base = c.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let digit = |rest: &str| matches!(rest, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³");
    (base.starts_with("COM") || base.starts_with("LPT")) && base.get(3..).is_some_and(digit)
}

pub fn safe_component(c: &str) -> bool {
    !c.is_empty() && c != "." && c != ".." && c.len() <= 255 && !c.ends_with(['.', ' ']) && !c.chars().any(|ch| (ch as u32) < 0x20 || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\')) && !reserved(c)
}

pub fn safe_path(path: &str) -> Option<String> {
    if path.starts_with(['/', '\\']) {
        return None;
    }
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect();
    (!parts.is_empty() && parts.iter().all(|c| safe_component(c))).then(|| parts.join("/"))
}

#[cfg(test)]
#[path = "../test/names.rs"]
mod tests;
