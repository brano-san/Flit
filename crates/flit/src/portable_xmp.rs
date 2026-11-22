use flit_core::portable_meta::Metadata;
use quick_xml::escape::{escape, resolve_xml_entity};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;

const RDF: Namespace<'static> = Namespace(b"http://www.w3.org/1999/02/22-rdf-syntax-ns#");
const DC: Namespace<'static> = Namespace(b"http://purl.org/dc/elements/1.1/");
const XMP: Namespace<'static> = Namespace(b"http://ns.adobe.com/xap/1.0/");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Other,
    Description,
    Rating,
    Comment,
    Subject,
    Alt,
    Bag,
    Entry,
}

fn kind(reader: &NsReader<&[u8]>, tag: &BytesStart<'_>) -> Shape {
    let (ns, local) = reader.resolver().resolve_element(tag.name());
    match (ns, local.as_ref()) {
        (ResolveResult::Bound(RDF), b"Description") => Shape::Description,
        (ResolveResult::Bound(RDF), b"Alt") => Shape::Alt,
        (ResolveResult::Bound(RDF), b"Bag") => Shape::Bag,
        (ResolveResult::Bound(RDF), b"li") => Shape::Entry,
        (ResolveResult::Bound(XMP), b"Rating") => Shape::Rating,
        (ResolveResult::Bound(DC), b"description") => Shape::Comment,
        (ResolveResult::Bound(DC), b"subject") => Shape::Subject,
        _ => Shape::Other,
    }
}

fn rating(text: &str) -> Option<u8> {
    let stars = text.trim().parse::<f64>().ok()?;
    if stars == -1.0 { return Some(0); }
    (0.0..=5.0).contains(&stars).then(|| (stars * 2.0).round() as u8)
}

fn rating_attribute(reader: &NsReader<&[u8]>, tag: &BytesStart<'_>, data: &mut Metadata) -> Option<bool> {
    for attribute in tag.attributes() {
        let a = attribute.ok()?;
        let (ns, local) = reader.resolver().resolve_attribute(a.key);
        if ns == ResolveResult::Bound(XMP) && local.as_ref() == b"Rating" {
            data.rating = rating(&a.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder()).ok()?)?;
            return Some(true);
        }
    }
    Some(false)
}

struct Node {
    kind: Shape,
    value: String,
    default_language: bool,
}

fn is_default(tag: &BytesStart<'_>) -> bool {
    tag.attributes().filter_map(Result::ok).any(|a| a.key.as_ref() == b"xml:lang" && a.value.as_ref() == b"x-default")
}

pub fn read(text: &str, before: &Metadata) -> Option<Metadata> {
    let mut reader = NsReader::from_str(text);
    let mut value = before.clone();
    let mut seen = false;
    let mut nodes: Vec<Node> = Vec::new();
    loop {
        match reader.read_event().ok()? {
            Event::Start(tag) => {
                let kind = kind(&reader, &tag);
                if kind == Shape::Description {
                    seen |= rating_attribute(&reader, &tag, &mut value)?;
                }
                if matches!(kind, Shape::Rating | Shape::Comment | Shape::Subject) {
                    seen = true;
                }
                if kind == Shape::Comment { value.comment.clear(); }
                if kind == Shape::Subject { value.tags.clear(); }
                nodes.push(Node { kind, value: String::new(), default_language: is_default(&tag) });
                if nodes.len() > 64 { return None; }
            }
            Event::Empty(tag) => {
                let kind = kind(&reader, &tag);
                if kind == Shape::Description {
                    seen |= rating_attribute(&reader, &tag, &mut value)?;
                }
                if kind == Shape::Comment {
                    value.comment.clear();
                    seen = true;
                }
                if kind == Shape::Subject {
                    value.tags.clear();
                    seen = true;
                }
            }
            Event::Text(s) => {
                if let Some(node) = nodes.last_mut().filter(|n| matches!(n.kind, Shape::Entry | Shape::Comment | Shape::Rating)) {
                    node.value.push_str(&s.xml10_content().ok()?);
                }
            }
            Event::CData(s) => {
                if let Some(node) = nodes.last_mut().filter(|n| matches!(n.kind, Shape::Entry | Shape::Comment | Shape::Rating)) {
                    node.value.push_str(&s.decode().ok()?);
                }
            }
            Event::GeneralRef(s) => {
                if let Some(node) = nodes.last_mut().filter(|n| matches!(n.kind, Shape::Entry | Shape::Comment | Shape::Rating)) {
                    if let Some(c) = s.resolve_char_ref().ok()? {
                        node.value.push(c);
                    } else {
                        node.value.push_str(resolve_xml_entity(&s.decode().ok()?)?);
                    }
                }
            }
            Event::End(_) => {
                let node = nodes.pop()?;
                let parent = nodes.last().map(|n| n.kind);
                let container = nodes.iter().rev().nth(1).map(|n| n.kind);
                match (node.kind, parent, container) {
                    (Shape::Rating, _, _) => value.rating = rating(&node.value)?,
                    (Shape::Entry, Some(Shape::Bag), Some(Shape::Subject)) => {
                        let tag = node.value.trim();
                        if !tag.is_empty() && !value.tags.iter().any(|known| known == tag) {
                            value.tags.push(tag.to_owned());
                        }
                    }
                    (Shape::Entry, Some(Shape::Alt), Some(Shape::Comment)) if node.default_language || value.comment.is_empty() => {
                        value.comment = node.value;
                    }
                    (Shape::Comment, _, _) if value.comment.is_empty() && !node.value.trim().is_empty() => value.comment = node.value,
                    _ => {}
                }
            }
            Event::DocType(_) => return None,
            Event::Eof => return (seen && nodes.is_empty() && value.valid()).then_some(value),
            _ => {}
        }
    }
}

pub fn write(value: &Metadata) -> String {
    let tags = value.tags.iter().map(|s| format!("<rdf:li>{}</rdf:li>", escape(s))).collect::<String>();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmp:Rating=\"{}\"><dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description><dc:subject><rdf:Bag>{tags}</rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>\n",
        f32::from(value.rating) / 2.0,
        escape(&value.comment),
    )
}
