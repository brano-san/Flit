use super::*;

fn titles(q: &str) -> Vec<&'static str> {
    matches(q).into_iter().map(|i| ACTIONS[i].title).collect()
}

#[test]
fn substrings_beat_fuzzy_matches_and_word_starts_win() {
    assert_eq!(score("", "Anything"), Some(0));
    assert!(score("tab", "New tab").unwrap() > score("ntb", "New tab").unwrap());
    assert!(score("new", "New tab").unwrap() > score("ew", "New tab").unwrap());
    assert!(score("tab", "New tab").unwrap() > 2000);
    assert!(score("ntb", "New tab").unwrap() < 2000);
    assert!(score("nt", "New tab").unwrap() > score("nb", "New tab").unwrap());
    assert_eq!(score("xyz", "New tab"), None);
    assert_eq!(score("  NEW TAB ", "New tab"), score("new tab", "New tab"));
}

#[test]
fn matches_rank_registered_titles() {
    assert_eq!(matches(""), (0..ACTIONS.len()).collect::<Vec<_>>());
    assert_eq!(titles("sort size"), ["Sort by size"]);
    assert_eq!(titles("refresh")[0], "Refresh");
    assert_eq!(titles("pin")[0], "Pin or unpin the current folder");
    assert_eq!(titles("filter")[0], "Filter items in this folder");
    assert!(titles("qqq").is_empty());
}
