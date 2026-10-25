use super::*;

fn titles(q: &str) -> Vec<&'static str> {
    matches(q).into_iter().map(|i| ACTIONS[i].title).collect()
}

#[test]
fn substrings_beat_fuzzy_matches_and_word_starts_win() {
    assert_eq!(score("", "Anything"), Some(0));
    assert!(score("tab", "New tab").unwrap() > score("nt", "New tab").unwrap());
    assert!(score("new", "New tab").unwrap() > score("ew", "New tab").unwrap());
    assert!(score("tab", "New tab").unwrap() > 2000);
    assert!(score("nt", "New tab").unwrap() < 2000);
    assert!(score("netab", "New tab").unwrap() > score("nt", "New tab").unwrap());
    assert_eq!(score("ntb", "New tab"), None, "fuzzy letters must start a word or follow the previous match");
    assert_eq!(score("sel", "Clear selection"), Some(3994));
    assert_eq!(score("so", "Clear selection"), None);
    assert!(score("so", "Show or hide hidden files").is_some());
    assert_eq!(score("xyz", "New tab"), None);
    assert_eq!(score("  NEW TAB ", "New tab"), score("new tab", "New tab"));
}

#[test]
fn matches_rank_registered_titles() {
    assert_eq!(matches(""), (0..ACTIONS.len()).collect::<Vec<_>>());
    assert_eq!(titles("sort size")[0], "Sort by size");
    assert_eq!(titles("refresh")[0], "Refresh");
    assert_eq!(titles("pin")[0], "Pin or unpin the current folder");
    assert_eq!(titles("filter")[0], "Filter items in this folder");
    assert!(titles("qqq").is_empty());
}

#[test]
fn commands_are_found_by_words_of_their_descriptions() {
    assert!(ACTIONS.iter().all(|a| !super::super::describe::description(a.id).is_empty()), "every command has a description");
    let dual = titles("dual pane");
    assert_eq!(dual[0], "Toggle dual pane", "a title match ranks first");
    assert!(dual.contains(&"Split vertically: open this folder in a pane on the right"));
    assert!(titles("trash").contains(&"Move to Recycle Bin"));
    assert!(titles("unzip").contains(&"Extract here"));
    assert!(titles("checksum").contains(&"Hashes…"));
    assert!(titles("trash qqq").is_empty(), "every word must match");
}
