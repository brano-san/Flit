pub fn set_stem(name: &str) -> Option<&str> {
    let (base, ext) = name.rsplit_once('.')?;
    let number = ext.len() == 3 && ext.bytes().all(|c| c.is_ascii_digit()) && ext != "000";
    (!base.is_empty() && (number || ext.eq_ignore_ascii_case("crc"))).then_some(base)
}
