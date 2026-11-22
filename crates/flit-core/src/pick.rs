use serde::{Deserialize, Serialize};
use crate::{FsPath, Location};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Open { multiple: bool, directories: bool },
    Save(Vec<u8>),
    SaveMany(Vec<Vec<u8>>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub label: String,
    pub patterns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mime_types: Vec<String>,
}

impl Filter {
    pub fn matches(&self, name: &[u8]) -> bool {
        self.matches_with_mime(name, None)
    }

    pub(crate) fn matches_with_mime(&self, name: &[u8], mime: Option<&[String]>) -> bool {
        if self.patterns.is_empty() && self.mime_types.is_empty() {
            return true;
        }
        if !self.mime_types.is_empty() && mime.is_none_or(|types| {
            self.mime_types.iter().any(|wanted| {
                wanted == "*/*" || types.iter().any(|actual| {
                    actual == wanted || wanted.strip_suffix("/*").is_some_and(|media| {
                        actual.split_once('/').is_some_and(|(kind, _)| kind == media)
                    })
                })
            })
        }) {
            return true;
        }
        if self.patterns.is_empty() {
            return false;
        }
        let display = crate::name::display(name);
        let display = if display.is_ascii() { display } else { display.to_lowercase().into() };
        self.patterns.iter().any(|pattern| crate::filter::picker_glob(pattern.as_bytes(), display.as_bytes()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub values: Vec<(String, String)>,
    pub selected: usize,
}

pub type Choices = Vec<(String, String)>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub mode: Mode,
    pub path: Option<Vec<u8>>,
    pub title: Option<String>,
    pub accept: Option<String>,
    pub filters: Vec<Filter>,
    pub filter: usize,
    pub choices: Vec<Choice>,
}

impl Default for Request {
    fn default() -> Self {
        Self { mode: Mode::Open { multiple: false, directories: false }, path: None, title: None, accept: None, filters: Vec::new(), filter: 0, choices: Vec::new() }
    }
}

impl Request {
    pub fn from_cli(args: Vec<Vec<u8>>) -> Option<Self> {
        let mut request = Self::default();
        let mut it = args.into_iter();
        let mut options = true;
        let mut multiple = false;
        while let Some(arg) = it.next() {
            match arg.as_slice() {
                b"--" if options => options = false,
                b"--save" if options && !request.saving() => request.mode = Mode::Save(it.next().filter(|name| !name.is_empty())?),
                b"--multiple" if options => multiple = true,
                b"--filter" if options && request.filters.is_empty() => {
                    let value = String::from_utf8(it.next()?).ok()?;
                    let patterns: Vec<_> = value.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase).collect();
                    if patterns.is_empty() {
                        return None;
                    }
                    request.filters.push(Filter { label: value, patterns, mime_types: Vec::new() });
                }
                arg if options && arg.starts_with(b"--") => return None,
                _ if request.path.is_none() => request.path = Some(arg),
                _ => return None,
            }
        }
        if multiple {
            if let Mode::Open { multiple, .. } = &mut request.mode {
                *multiple = true;
            } else {
                return None;
            }
        }
        Some(request)
    }

    pub fn saving(&self) -> bool {
        !matches!(self.mode, Mode::Open { .. })
    }

    pub fn directories(&self) -> bool {
        matches!(self.mode, Mode::Open { directories: true, .. } | Mode::SaveMany(_))
    }
}

pub fn local_path(location: &Location) -> Option<&FsPath> {
    use crate::path::PathKind;
    (location.provider() == crate::ProviderId::Local && matches!(location.path().kind(), PathKind::Unix | PathKind::Drive | PathKind::Unc | PathKind::Verbatim)).then(|| location.path())
}

pub fn valid_name(name: &[u8], unix: bool) -> bool {
    if name.is_empty() || matches!(name, b"." | b"..") || name.iter().any(|b| matches!(b, 0 | b'/')) {
        return false;
    }
    unix || (!name.ends_with(b".") && !name.ends_with(b" ") && !name.iter().any(|&b| b < 32 || b"\\:*?\"<>|".contains(&b)))
}

#[cfg(test)]
#[path = "../test/pick.rs"]
mod tests;
