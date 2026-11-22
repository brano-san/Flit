const MAX_PAGES: usize = 100_000;
pub const PAGE_PX: u32 = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

pub fn sizes(output: &[u8]) -> Option<Vec<Option<Size>>> {
    let text = String::from_utf8_lossy(output);
    let count: usize = text.lines().find_map(|line| line.strip_prefix("Pages:"))?.trim().parse().ok()?;
    if !(1..=MAX_PAGES).contains(&count) {
        return None;
    }
    let mut sizes = vec![None; count];
    let mut rotation = vec![0i32; count];
    for line in text.lines() {
        let Some((key, value)) = line.strip_prefix("Page ").and_then(|l| l.split_once(':')) else { continue };
        let mut parts = key.split_ascii_whitespace();
        let Some(first) = parts.next() else { continue };
        let (page, field) = match parts.next() {
            Some(field) => (first.parse::<usize>().ok()?, field),
            None => (1, first),
        };
        let index = page.checked_sub(1).filter(|&i| i < count)?;
        match field {
            "size" => sizes[index] = page_size(value),
            "rot" => rotation[index] = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    Some(
        sizes
            .into_iter()
            .zip(rotation)
            .map(|(s, a)| {
                let Size { width, height } = s?;
                Some(if matches!(a.rem_euclid(360), 90 | 270) { Size { width: height, height: width } } else { Size { width, height } })
            })
            .collect(),
    )
}

fn page_size(value: &str) -> Option<Size> {
    let mut parts = value.split_ascii_whitespace();
    let width: f32 = parts.next()?.parse().ok()?;
    if parts.next()? != "x" {
        return None;
    }
    let height: f32 = parts.next()?.parse().ok()?;
    (parts.next()? == "pts" && width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0).then_some(Size { width, height })
}
