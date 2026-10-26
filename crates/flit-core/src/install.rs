use crate::name;

pub fn font_value_name(file_name: &[u8]) -> Option<String> {
    let ext = name::ext_len(file_name);
    let stem = name::display(&file_name[..file_name.len() - ext - usize::from(ext > 0)]).trim().to_owned();
    let kind = match name::display(&file_name[file_name.len() - ext..]).to_ascii_lowercase().as_str() {
        "ttf" | "ttc" => "TrueType",
        "otf" => "OpenType",
        _ => return None,
    };
    (!stem.is_empty()).then(|| format!("{stem} ({kind})"))
}

pub fn is_font(file_name: &[u8]) -> bool {
    font_value_name(file_name).is_some()
}

#[cfg(test)]
#[path = "../test/install.rs"]
mod tests;
