pub(crate) fn attribute_range(tag: &[u8], name: &[u8]) -> Option<std::ops::Range<usize>> {
    let mut rest = &tag[tag.iter().position(u8::is_ascii_whitespace)?..];
    loop {
        rest = rest.trim_ascii_start();
        let key_end = rest.iter().position(|b| b.is_ascii_whitespace() || *b == b'=')?;
        let value = rest[key_end..].trim_ascii_start().strip_prefix(b"=")?.trim_ascii_start();
        let (&quote, value) = value.split_first()?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        let end = value.iter().position(|&b| b == quote)?;
        if &rest[..key_end] == name {
            let start = tag.len() - value.len();
            return Some(start..start + end);
        }
        rest = &value[end + 1..];
    }
}

pub(crate) fn attribute<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    Some(&tag[attribute_range(tag, name)?])
}

fn numeric_xml_reference(s: &[u8]) -> Option<(usize, char)> {
    let number = s.strip_prefix(b"&#")?;
    let (digits, radix) = number.strip_prefix(b"x").map_or((number, 10), |hex| (hex, 16));
    let end = digits.iter().position(|b| !b.is_ascii_hexdigit())?;
    if digits[end] != b';' {
        return None;
    }
    let code = u32::from_str_radix(std::str::from_utf8(&digits[..end]).ok()?, radix).ok()?;
    let c = char::from_u32(code)?;
    Some((s.len() - digits.len() + end + 1, c))
}

pub(crate) fn unescape_xml(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let entity = [(&b"&amp;"[..], b'&'), (b"&lt;", b'<'), (b"&gt;", b'>'), (b"&quot;", b'"'), (b"&apos;", b'\'')].into_iter().find(|(e, _)| s[i..].starts_with(e));
        if let Some((e, c)) = entity {
            out.push(c);
            i += e.len();
        } else if let Some((len, c)) = numeric_xml_reference(&s[i..]) {
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            i += len;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

pub(crate) fn bookmark_tags(input: &[u8]) -> impl Iterator<Item = &[u8]> {
    input.split(|&b| b == b'<').filter(|t| t.starts_with(b"bookmark "))
}

pub(crate) fn bookmark_href(tag: &[u8]) -> Option<Vec<u8>> {
    attribute(tag, b"href").map(unescape_xml)
}
