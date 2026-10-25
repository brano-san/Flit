use std::borrow::Cow;
use std::fmt::{self, Display, Write};
use std::sync::atomic::{AtomicU8, Ordering};

include!(concat!(env!("OUT_DIR"), "/strings.rs"));

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(u8)]
pub enum Lang {
    #[default]
    En,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.split(['-', '_']).next().unwrap_or_default();
        Lang::ALL.into_iter().find(|l| primary.eq_ignore_ascii_case(l.code()))
    }

    fn from_u8(v: u8) -> Lang {
        if v == Lang::Ru as u8 { Lang::Ru } else { Lang::En }
    }

    fn form(self, n: u64) -> usize {
        match self {
            Lang::En => usize::from(n != 1),
            Lang::Ru => match (n % 10, n % 100) {
                (1, r) if r != 11 => 0,
                (2..=4, r) if !(12..=14).contains(&r) => 1,
                _ => 2,
            },
        }
    }

    pub fn group_separator(self) -> &'static str {
        match self {
            Lang::En => ",",
            Lang::Ru => "\u{A0}",
        }
    }

    pub fn decimal_separator(self) -> char {
        match self {
            Lang::En => '.',
            Lang::Ru => ',',
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);
static SYSTEM: AtomicU8 = AtomicU8::new(Lang::En as u8);

#[cfg(test)]
thread_local! {
    static LOCAL: std::cell::Cell<Lang> = const { std::cell::Cell::new(Lang::En) };
}

#[cfg(not(test))]
#[inline]
pub fn lang() -> Lang {
    Lang::from_u8(CURRENT.load(Ordering::Relaxed))
}

#[cfg(test)]
#[inline]
pub fn lang() -> Lang {
    LOCAL.with(|l| l.get())
}

pub fn set_lang(l: Lang) -> bool {
    #[cfg(test)]
    LOCAL.with(|c| c.set(l));
    CURRENT.swap(l as u8, Ordering::Relaxed) != l as u8
}

pub fn search_langs() -> &'static [Lang] {
    match lang() {
        Lang::En => &[Lang::En],
        Lang::Ru => &[Lang::En, Lang::Ru],
    }
}

pub fn system_lang() -> Lang {
    Lang::from_u8(SYSTEM.load(Ordering::Relaxed))
}

pub fn set_system_lang(l: Lang) {
    SYSTEM.store(l as u8, Ordering::Relaxed);
}

impl S {
    #[inline]
    pub fn t(self) -> &'static str {
        S_TABLE[lang() as usize][self as usize]
    }

    #[inline]
    pub fn en(self) -> &'static str {
        S_TABLE[Lang::En as usize][self as usize]
    }

    #[inline]
    pub fn in_lang(self, l: Lang) -> &'static str {
        S_TABLE[l as usize][self as usize]
    }

    pub fn key(self) -> &'static str {
        S_KEYS[self as usize]
    }
}

impl Display for S {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.t())
    }
}

impl P {
    #[inline]
    pub fn pick(self, n: u64) -> &'static str {
        self.pick_in(lang(), n)
    }

    pub fn pick_in(self, l: Lang, n: u64) -> &'static str {
        let forms = P_TABLE[l as usize][self as usize];
        forms[l.form(n).min(forms.len() - 1)]
    }

    pub fn forms(self, l: Lang) -> &'static [&'static str] {
        P_TABLE[l as usize][self as usize]
    }

    pub fn key(self) -> &'static str {
        P_KEYS[self as usize]
    }
}

pub struct Num(pub u64);

impl Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        let mut n = self.0;
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        let digits = &buf[i..];
        let sep = lang().group_separator();
        for (k, d) in digits.iter().enumerate() {
            if k > 0 && (digits.len() - k) % 3 == 0 {
                f.write_str(sep)?;
            }
            f.write_char(*d as char)?;
        }
        Ok(())
    }
}

pub fn num(n: u64) -> Num {
    Num(n)
}

pub fn decimal(out: &mut impl Write, v: f64, digits: usize) {
    let sep = lang().decimal_separator();
    if sep == '.' {
        let _ = write!(out, "{v:.digits$}");
        return;
    }
    let mut buf = Stack::default();
    let _ = write!(buf, "{v:.digits$}");
    for c in buf.as_str().chars() {
        let _ = out.write_char(if c == '.' { sep } else { c });
    }
}

struct Stack {
    bytes: [u8; 48],
    len: usize,
}

impl Default for Stack {
    fn default() -> Self {
        Self { bytes: [0; 48], len: 0 }
    }
}

impl Stack {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl Write for Stack {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

pub fn fill_into(out: &mut impl Write, template: &str, args: &[(&str, &dyn Display)]) {
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let _ = out.write_str(&rest[..open]);
        let tail = &rest[open + 1..];
        match tail.find('}') {
            Some(close) => {
                let name = &tail[..close];
                match args.iter().find(|(n, _)| *n == name) {
                    Some((_, v)) => {
                        let _ = write!(out, "{v}");
                    }
                    None => {
                        let _ = write!(out, "{{{name}}}");
                    }
                }
                rest = &tail[close + 1..];
            }
            None => {
                let _ = out.write_str(&rest[open..]);
                rest = "";
            }
        }
    }
    let _ = out.write_str(rest);
}

pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    fill_into(&mut out, template, args);
    out
}

pub fn placeholders(template: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('}') else { break };
        out.push(&tail[..close]);
        rest = &tail[close + 1..];
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn captures<'a>(template: &'static str, msg: &'a str) -> Option<Vec<(&'static str, &'a str)>> {
    let mut pieces = Vec::new();
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let tail = &rest[open + 1..];
        let close = tail.find('}')?;
        pieces.push(&rest[..open]);
        names.push(&tail[..close]);
        rest = &tail[close + 1..];
    }
    pieces.push(rest);
    if names.is_empty() {
        return (msg == template).then(Vec::new);
    }
    let mut at = msg.strip_prefix(pieces[0])?;
    let mut out = Vec::with_capacity(names.len());
    for (k, name) in names.iter().enumerate() {
        let next = pieces[k + 1];
        let end = if k + 1 == names.len() {
            if !at.ends_with(next) {
                return None;
            }
            at.len() - next.len()
        } else {
            if next.is_empty() {
                return None;
            }
            at.find(next)?
        };
        if end == 0 {
            return None;
        }
        out.push((*name, &at[..end]));
        at = &at[end + next.len()..];
    }
    Some(out)
}

pub fn localize(msg: &str) -> Cow<'_, str> {
    let l = lang();
    if l == Lang::En || msg.is_empty() {
        return Cow::Borrowed(msg);
    }
    for &k in errors() {
        let en = k.en();
        if let Some(args) = captures(en, msg) {
            let localized: Vec<(&str, String)> = args.iter().map(|&(n, v)| (n, localize(v).into_owned())).collect();
            let refs: Vec<(&str, &dyn Display)> = localized.iter().map(|(n, v)| (*n, v as &dyn Display)).collect();
            return Cow::Owned(fill(k.in_lang(l), &refs));
        }
    }
    Cow::Borrowed(msg)
}

fn errors() -> &'static [S] {
    static ERRORS: std::sync::OnceLock<Vec<S>> = std::sync::OnceLock::new();
    ERRORS.get_or_init(|| {
        let mut v: Vec<S> = S::ALL.into_iter().filter(|s| s.key().starts_with("errors.")).collect();
        v.sort_by_key(|s| std::cmp::Reverse(literal_len(s.en())));
        v
    })
}

fn literal_len(template: &str) -> usize {
    template.len() - placeholders(template).iter().map(|p| p.len() + 2).sum::<usize>()
}

#[macro_export]
macro_rules! tr {
    ($k:expr $(, $name:ident = $v:expr)* $(,)?) => {
        $crate::i18n::fill($k.t(), &[$((stringify!($name), &$v as &dyn ::std::fmt::Display)),*])
    };
}

#[macro_export]
macro_rules! tr_en {
    ($k:expr $(, $name:ident = $v:expr)* $(,)?) => {
        $crate::i18n::fill($k.en(), &[$((stringify!($name), &$v as &dyn ::std::fmt::Display)),*])
    };
}

#[macro_export]
macro_rules! trn {
    ($k:expr, $n:expr $(, $name:ident = $v:expr)* $(,)?) => {{
        let n: u64 = $n as u64;
        $crate::i18n::fill($k.pick(n), &[("n", &$crate::i18n::num(n) as &dyn ::std::fmt::Display) $(, (stringify!($name), &$v as &dyn ::std::fmt::Display))*])
    }};
}

#[macro_export]
macro_rules! trn_into {
    ($out:expr, $k:expr, $n:expr $(, $name:ident = $v:expr)* $(,)?) => {{
        let n: u64 = $n as u64;
        $crate::i18n::fill_into($out, $k.pick(n), &[("n", &$crate::i18n::num(n) as &dyn ::std::fmt::Display) $(, (stringify!($name), &$v as &dyn ::std::fmt::Display))*])
    }};
}

#[macro_export]
macro_rules! tr_into {
    ($out:expr, $k:expr $(, $name:ident = $v:expr)* $(,)?) => {
        $crate::i18n::fill_into($out, $k.t(), &[$((stringify!($name), &$v as &dyn ::std::fmt::Display)),*])
    };
}

#[cfg(test)]
#[path = "../test/i18n.rs"]
mod tests;
