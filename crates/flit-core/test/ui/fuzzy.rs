use super::*;

fn spans(q: &str, text: &str) -> Vec<String> {
    fuzzy(q, text).expect("a match").ranges.into_iter().map(|r| text[r].to_owned()).collect()
}

fn score(q: &str, text: &str) -> i32 {
    fuzzy(q, text).map_or(i32::MIN, |m| m.score)
}

#[test]
fn letters_match_in_order_with_gaps_and_ignore_case() {
    assert!(fuzzy("ntb", "New tab").is_some());
    assert!(fuzzy("NEWTAB", "new tab").is_some());
    assert!(fuzzy("tsb", "Toggle Subfolders").is_some());
    assert_eq!(fuzzy("bat", "New tab"), None, "letters must keep their order");
    assert_eq!(fuzzy("xyz", "New tab"), None);
    assert_eq!(fuzzy("tabs", "tab"), None);
    assert_eq!(fuzzy("", "anything"), Some(Match { score: 0, ranges: Vec::new() }));
    assert_eq!(fuzzy("  new  tab ", "New tab").map(|m| m.score), fuzzy("newtab", "New tab").map(|m| m.score));
}

#[test]
fn highlight_ranges_cover_the_matched_letters() {
    assert_eq!(spans("tab", "New tab"), ["tab"]);
    assert_eq!(spans("ntb", "New tab"), ["N", "t", "b"]);
    assert_eq!(spans("sub", "Toggle Subfolders"), ["Sub"]);
    assert_eq!(spans("ts", "Toggle Subfolders"), ["T", "S"], "word starts beat letters inside words");
    assert_eq!(spans("ü", "Größe über"), ["ü"]);
    let m = fuzzy("gRÖ", "Größe").unwrap();
    assert_eq!(m.ranges, std::iter::once(0..4).collect::<Vec<_>>(), "ranges are byte offsets on char boundaries");
}

#[test]
fn contiguous_runs_and_word_starts_rank_higher() {
    assert!(score("tab", "New tab") > score("ntb", "New tab"));
    assert!(score("new", "New tab") > score("ew", "New tab"));
    assert!(score("sel", "Select all") > score("sel", "Clear selection"), "a match at the start ranks first");
    assert!(score("sel", "Clear selection") > score("sel", "Close all files"), "a run beats scattered letters");
    assert!(score("ts", "Toggle Subfolders") > score("ts", "Tests"), "two word starts beat a start and a later letter");
    assert!(score("sp", "Split Right") > score("sp", "Show preview"));
}
