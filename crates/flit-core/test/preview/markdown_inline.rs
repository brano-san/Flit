use super::*;

#[test]
fn links_keep_labels_and_leave_sentence_punctuation_outside() {
    let d = markdown("[inline](private) [reference][ref] <https://one.test> (https://two.test/a(b)). `https://code.test`\n\n[ref]: hidden");
    let links: Vec<_> = d.runs.iter().filter(|run| run.tone == Tone::Link).map(|run| d.run_text(run)).collect();
    assert_eq!(links, ["inline", "reference", "https://one.test", "https://two.test/a(b)"]);
    assert_eq!(d.text, "inline reference https://one.test (https://two.test/a(b)). https://code.test");
}

#[test]
fn strike_coexists_with_bold_code_and_links() {
    let d = markdown("~~**old** [site](path) `cmd`~~ new");
    assert!(d.runs.iter().any(|r| d.run_text(r) == "old" && r.bold && r.strike));
    assert!(d.runs.iter().any(|r| d.run_text(r) == "site" && r.tone == Tone::Link && r.strike));
    assert!(d.runs.iter().any(|r| d.run_text(r) == "cmd" && r.mono && r.strike));
    assert!(d.runs.iter().any(|r| d.run_text(r) == " new" && !r.strike));
}

#[test]
fn images_use_dim_alt_text_or_the_filename() {
    let d = markdown("![view](https://images.test/hidden.png) and ![](dir/pic.png?v=2#top)");
    assert_eq!(d.text, "🖼 view and 🖼 pic.png");
    assert!(d.runs.iter().filter(|r| r.tone == Tone::Dim).map(|r| d.run_text(r)).eq(["🖼 view", "🖼 pic.png"]));
}

#[test]
fn html_preserves_visible_content_and_keyboard_style() {
    let d = markdown("before<Br/>after <kbd class='key'>Enter</kbd><!-- inline -->.\n\n<details title='1 > 0'>\n<summary>Summary</summary>\n<!-- invisible\nstill invisible -->visible <kbd>Alt</kbd>\n</details>");
    assert!(d.text.starts_with("before\nafter Enter."));
    assert!(d.text.contains("Summary") && d.text.contains("visible Alt"));
    assert!(!d.text.contains("invisible") && !d.text.contains('<') && !d.text.contains('>'));
    assert!(d.runs.iter().filter(|r| r.mono).map(|r| d.run_text(r)).eq(["Enter", "Alt"]));
}
