use super::*;
use crate::preview::Run;

#[path = "markdown_inline.rs"]
mod inline_cases;

fn paras(d: &Doc) -> Vec<(Block, String)> {
    d.paras.iter().map(|p| (p.block, d.runs[p.runs.start as usize..p.runs.end as usize].iter().map(|r| d.run_text(r)).collect())).collect()
}

fn runs(d: &Doc, k: usize) -> Vec<Run> {
    d.runs[d.paras[k].runs.start as usize..d.paras[k].runs.end as usize].to_vec()
}

#[test]
fn blocks() {
    let d = markdown("# Title #\n\nSome *em* text\ncontinues.\n\n- one\n  more\n- [x] done\n\n2. two\n\n> quoted\n> line\n\n---\n\n```rust\nfn x() {}\n```\nSetext\n===\n| a | b |\n|---|---|\n| 1 | 2 |");
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
            (Block::Table(0), "ab12".into()),
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
    assert_eq!(paras(&d)[0].1, "*not em* 🖼 alt https://flit.dev");
}

#[test]
fn closing_hashes_must_be_separated_from_the_heading() {
    let d = markdown("## C# ##");
    assert_eq!(paras(&d), [(Block::Heading(2), "C#".into())]);
    assert_eq!(markdown("## C#").text, "C#");
}

#[test]
#[ignore = "manual parser timing"]
fn large_markdown_timing() {
    let sample = "# Project\n\nA README paragraph with **bold**, `code`, [docs](https://example.com) and several ordinary words.\n\n- [x] build\n- verify\n\n| Name | Result |\n| --- | ---: |\n| Core | 123 |\n\n```rust\nfn main() {}\n```\n\n";
    let input = sample.repeat(1_500_000 / sample.len());
    let mut timings = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let doc = markdown(std::hint::black_box(&input));
        timings.push(start.elapsed());
        assert!(std::hint::black_box(doc).paras.len() > 10_000);
    }
    timings.sort();
    eprintln!("{} bytes, median of five parses: {:?}", input.len(), timings[2]);
}

#[test]
fn table_cells_keep_their_alignment_text_and_inline_formatting() {
    use crate::preview::Alignment::{Center, Left, Right};

    let d = markdown("Name | **Status** | Score\n:--- | :---: | ---:\nfoo\\|bar | **ok** | `10`\nnext | | 2");
    assert_eq!(d.paras.len(), 1);
    let Block::Table(i) = d.paras[0].block else { panic!("expected a table") };
    let table = &d.tables[i];
    assert_eq!(table.align, [Left, Center, Right]);
    let cells: Vec<Vec<String>> = table.rows.iter().map(|row| {
        row.iter().map(|cell| cell.clone().map(|ri| d.run_text(&d.runs[ri as usize])).collect()).collect()
    }).collect();
    assert_eq!(cells, [["Name", "Status", "Score"], ["foo|bar", "ok", "10"], ["next", "", "2"]]);
    assert!(table.rows[0].iter().flat_map(Clone::clone).all(|ri| d.runs[ri as usize].bold));
    assert!(d.runs[table.rows[1][1].start as usize].bold);
    assert!(d.runs[table.rows[1][2].start as usize].mono);
}
