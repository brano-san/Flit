use flit_core::preview::{self, Doc, PrettyError, PrettyFormat, PreviewRequest, PRETTY_FILE_LIMIT};
use quick_xml::events::Event;
use serde::Deserialize;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use toml_edit::{visit_mut, Array, KeyMut, Item, RawString};

pub(crate) const COMMAND: &str = "--preview-pretty-worker";
const MAX_OUTPUT: usize = 16 << 20;
const MAX_DEPTH: usize = 128;

pub(crate) fn load(req: &PreviewRequest) -> Option<Result<Doc, PrettyError>> {
    let current = || req.cancel.is_current(req.gen);
    if !current() {
        return None;
    }
    let path = super::std_path(&req.path);
    let result = super::child::run(COMMAND, &[path.as_os_str()], MAX_OUTPUT as u64, Duration::from_secs(3), None, current);
    if !current() {
        return None;
    }
    Some(match result {
        Some((status, bytes)) if status.success() => Ok(preview::build(req.kind, &bytes)),
        Some((status, _)) if status.code() == Some(2) => Err(PrettyError::Invalid),
        Some((status, _)) if status.code() == Some(3) => Err(PrettyError::TooLarge),
        _ => Err(PrettyError::Unavailable),
    })
}

pub(crate) fn serve(path: Option<&OsStr>) -> i32 {
    match path.map(Path::new).ok_or(PrettyError::Unavailable).and_then(format_file) {
        Ok(text) => i32::from(std::io::stdout().lock().write_all(text.as_bytes()).is_err()),
        Err(PrettyError::Invalid) => 2,
        Err(PrettyError::TooLarge) => 3,
        Err(PrettyError::Unavailable) => 1,
    }
}

pub(super) fn format_file(path: &Path) -> Result<String, PrettyError> {
    let format = path.extension().and_then(|e| PrettyFormat::for_ext(e.as_encoded_bytes())).ok_or(PrettyError::Invalid)?;
    let file = super::regular_file(path).ok_or(PrettyError::Unavailable)?;
    let meta = file.metadata().map_err(|_| PrettyError::Unavailable)?;
    if !meta.is_file() {
        return Err(PrettyError::Unavailable);
    }
    if meta.len() > PRETTY_FILE_LIMIT {
        return Err(PrettyError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(PRETTY_FILE_LIMIT + 1).read_to_end(&mut bytes).map_err(|_| PrettyError::Unavailable)?;
    if bytes.len() as u64 > PRETTY_FILE_LIMIT {
        return Err(PrettyError::TooLarge);
    }
    let text = preview::decode(&bytes);
    let formatted = match format {
        PrettyFormat::Json => json(&text),
        PrettyFormat::Yaml => pretty_yaml::format_text(&text, &Default::default()).map_err(|_| PrettyError::Invalid),
        PrettyFormat::Toml => toml(&text),
        PrettyFormat::Xml => xml(&text).ok_or(PrettyError::Invalid),
    }?;
    if formatted.len() > MAX_OUTPUT {
        return Err(PrettyError::TooLarge);
    }
    Ok(formatted)
}

fn newline(out: &mut Vec<u8>, depth: usize) {
    if !out.is_empty() {
        out.push(b'\n');
    }
    out.resize(out.len() + depth * 2, b' ');
}

fn json(text: &str) -> Result<String, PrettyError> {
    let mut parser = serde_json::Deserializer::from_str(text);
    serde::de::IgnoredAny::deserialize(&mut parser).and_then(|_| parser.end()).map_err(|_| PrettyError::Invalid)?;
    let mut out = Vec::with_capacity(text.len());
    let mut depth = 0;
    let (mut quoted, mut escaped, mut line) = (false, false, false);
    let mut previous = 0;
    for b in text.bytes() {
        if quoted {
            out.push(b);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
            continue;
        }
        if b.is_ascii_whitespace() {
            continue;
        }
        match b {
            b'}' | b']' => {
                depth -= 1;
                if !matches!(previous, b'{' | b'[') {
                    newline(&mut out, depth);
                }
            }
            _ if line => newline(&mut out, depth),
            _ => {}
        }
        line = matches!(b, b'{' | b'[' | b',');
        if matches!(b, b'{' | b'[') {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(PrettyError::TooLarge);
            }
        }
        quoted = b == b'"';
        out.push(b);
        if b == b':' {
            out.push(b' ');
        }
        if out.len() > MAX_OUTPUT {
            return Err(PrettyError::TooLarge);
        }
        previous = b;
    }
    out.push(b'\n');
    String::from_utf8(out).map_err(|_| PrettyError::Invalid)
}

fn inline_space(raw: Option<&RawString>) -> bool {
    raw.and_then(RawString::as_str).is_none_or(|s| s.bytes().all(|b| matches!(b, b' ' | b'\t')))
}

struct TomlFormat;

impl visit_mut::VisitMut for TomlFormat {
    fn visit_table_like_kv_mut(&mut self, mut key: KeyMut<'_>, item: &mut Item) {
        if let Some(value) = item.as_value_mut() {
            if inline_space(key.leaf_decor_mut().suffix()) {
                key.leaf_decor_mut().set_suffix(" ");
            }
            if inline_space(value.decor().prefix()) {
                value.decor_mut().set_prefix(" ");
            }
        }
        visit_mut::visit_table_like_kv_mut(self, key, item);
    }

    fn visit_array_mut(&mut self, array: &mut Array) {
        if inline_space(Some(array.trailing())) && array.iter().all(|v| inline_space(v.decor().prefix()) && inline_space(v.decor().suffix())) {
            array.fmt();
        }
        visit_mut::visit_array_mut(self, array);
    }
}

fn toml(text: &str) -> Result<String, PrettyError> {
    use visit_mut::VisitMut;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| PrettyError::Invalid)?;
    TomlFormat.visit_document_mut(&mut doc);
    Ok(doc.to_string())
}

#[derive(Default)]
struct XmlElement {
    preserve: bool,
    children: bool,
}

fn significant(text: &[u8]) -> bool {
    text.iter().any(|b| !matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
}

fn xml_elements(text: &str) -> Option<Vec<XmlElement>> {
    let mut parser = quick_xml::Reader::from_str(text);
    let mut elements: Vec<XmlElement> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut root = false;
    loop {
        let event = parser.read_event().ok()?;
        match event {
            Event::Start(ref tag) | Event::Empty(ref tag) => {
                if let Some(&parent) = stack.last() {
                    elements[parent].children = true;
                } else if std::mem::replace(&mut root, true) {
                    return None;
                }
                let mut preserve = false;
                for att in tag.attributes() {
                    let att = att.ok()?;
                    if att.key.as_ref() == b"xml:space" {
                        preserve = att.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, parser.decoder()).ok()? == "preserve";
                    }
                }
                if matches!(event, Event::Start(_)) {
                    if stack.len() == MAX_DEPTH {
                        return None;
                    }
                    stack.push(elements.len());
                    elements.push(XmlElement { preserve, children: false });
                }
            }
            Event::End(_) => {
                stack.pop()?;
            }
            Event::Text(ref t) if significant(t) => elements[*stack.last()?].preserve = true,
            Event::CData(_) | Event::GeneralRef(_) => elements[*stack.last()?].preserve = true,
            Event::Comment(_) | Event::PI(_) => {
                if let Some(&parent) = stack.last() {
                    elements[parent].children = true;
                }
            }
            Event::Eof => return (root && stack.is_empty()).then_some(elements),
            _ => {}
        }
    }
}

fn xml(text: &str) -> Option<String> {
    let elements = xml_elements(text)?;
    let mut elements = elements.into_iter();
    let mut parser = quick_xml::Reader::from_str(text);
    let mut out = quick_xml::Writer::new(Vec::with_capacity(text.len()));
    let mut stack = Vec::new();
    loop {
        let event = parser.read_event().ok()?;
        let indent = stack.last().copied().unwrap_or(true);
        match &event {
            Event::Start(_) => {
                if indent {
                    newline(out.get_mut(), stack.len());
                }
                let element = elements.next()?;
                stack.push(indent && element.children && !element.preserve);
            }
            Event::End(_) => {
                if stack.pop()? {
                    newline(out.get_mut(), stack.len());
                }
            }
            Event::Text(t) if indent && !significant(t) => continue,
            Event::Eof => break,
            _ if indent => newline(out.get_mut(), stack.len()),
            _ => {}
        }
        out.write_event(event).ok()?;
        if out.get_ref().len() > MAX_OUTPUT {
            return None;
        }
    }
    out.get_mut().push(b'\n');
    String::from_utf8(out.into_inner()).ok()
}
