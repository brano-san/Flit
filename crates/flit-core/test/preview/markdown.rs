use super::*;
use crate::preview::Run;

fn paras(d: &Doc) -> Vec<(Block, String)> {
    d.paras.iter().map(|p| (p.block, d.runs[p.runs.start as usize..p.runs.end as usize].iter().map(|r| d.run_text(r)).collect())).collect()
}

fn runs(d: &Doc, k: usize) -> Vec<Run> {
    d.runs[d.paras[k].runs.start as usize..d.paras[k].runs.end as usize].to_vec()
}

#[test]
fn blocks() {
    let d = markdown("# Title #\n\nSome *em* text\ncontinues.\n\n- one\n  more\n- [x] done\n2. two\n\n> quoted\n> line\n\n---\n\n```rust\nfn x() {}\n```\nSetext\n===\n| a | b |\n|---|---|\n| 1 | 2 |");
    assert_eq!(
        paras(&d),
        [
            (Block::Heading(1), "Title".into()),
            (Block::Body, "Some em text continues.".into()),
            (Block::Item { depth: 0 }, "\u{2022} one more".into()),
            (Block::Item { depth: 0 }, "\u{2611} done".into()),
            (Block::Item { depth: 0 }, "2. two".into()),
            (Block::Quote, "quoted line".into()),
            (Block::Rule, String::new()),
            (Block::Code, "fn x() {}".into()),
            (Block::Heading(1), "Setext".into()),
            (Block::Body, "a  \u{2502}  b".into()),
            (Block::Body, "1  \u{2502}  2".into()),
        ]
    );
    assert!(d.wrap && !d.mono);
    assert!(runs(&d, 7).iter().any(|r| r.tone == Tone::Keyword && r.mono));
}

#[test]
fn inline_styles() {
    let d = markdown("Use **bold** and `code` with [link](http://x) and snake_case_name and 2*3.");
    let r = runs(&d, 0);
    let text = |r: &Run| d.run_text(r).to_owned();
    assert!(r.iter().any(|r| r.bold && text(r) == "bold"));
    assert!(r.iter().any(|r| r.mono && r.tone == Tone::String && text(r) == "code"));
    assert!(r.iter().any(|r| r.tone == Tone::Link && text(r) == "link"));
    assert_eq!(paras(&d)[0].1, "Use bold and code with link and snake_case_name and 2*3.");
}

#[test]
fn escapes_images_and_autolinks() {
    let d = markdown(r"\*not em\* ![alt](a.png) <https://flit.dev>");
    assert_eq!(paras(&d)[0].1, "*not em* alt https://flit.dev");
}
