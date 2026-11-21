use super::*;
use crate::temp::Temp;
use std::sync::mpsc;

fn files(name: &str) -> Vec<(PathBuf, String)> {
    [("plain", ""), (name, "Conditions=exec probe;")]
        .into_iter()
        .map(|(label, cond)| (Path::new("/actions").join(label), format!("[Nemo Action]\nName={label}\nExec=tool\nSelection=any\nExtensions=any;\n{cond}")))
        .collect()
}

#[test]
fn opening_another_menu_while_a_condition_runs_shows_plain_actions_immediately() {
    let cache = Arc::new(Mutex::new(NemoList::default()));
    let (started, checking) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let first = cached_nemo(&cache, files("checked"), "", Instant::now(), move |actions| {
        started.send(()).unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(4));
        actions.to_vec()
    });
    let labels = |actions: &[Action]| {
        let (items, _) = menu(actions, &[], Path::new("/tmp"));
        items.into_iter().filter_map(|item| match item { MenuItem::Command { label, .. } => Some(label), _ => None }).collect::<Vec<_>>()
    };
    assert_eq!(labels(&first), ["plain"]);
    checking.recv_timeout(Duration::from_secs(2)).unwrap();
    let (menu_ready, read_menu) = mpsc::channel();
    let second = std::thread::spawn(move || {
        let visible = cached_nemo(&cache, files("checked"), "", Instant::now(), |_| panic!("duplicate condition check"));
        menu_ready.send(labels(&visible)).unwrap();
    });
    let result = read_menu.recv_timeout(Duration::from_secs(1));
    let _ = release.send(());
    second.join().unwrap();
    assert_eq!(result.unwrap(), ["plain"]);
}

#[test]
fn a_refresh_after_thirty_seconds_keeps_the_previous_results_until_it_finishes() {
    let now = Instant::now();
    let mut list = NemoList::default();
    let first = list.request(files("checked"), "", now).unwrap();
    list.finish(&first, first.to_vec(), now);
    assert!(list.request(files("checked"), "", now + Duration::from_secs(30)).is_none());
    let later = now + Duration::from_secs(31);
    let refresh = list.request(files("checked"), "", later).unwrap();
    assert_eq!(list.visible.len(), 2);
    assert!(list.request(files("checked"), "", later).is_none());
    list.finish(&refresh, vec![refresh[0].clone()], later);
    assert_eq!(list.visible.len(), 1);
    assert!(list.request(files("checked"), "", later).is_none());
}

#[test]
fn editing_an_action_file_discards_its_cached_result_and_ignores_the_previous_worker() {
    let data = Temp::new("nemo-cache-files");
    let dir = data.path().join("nemo/actions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("checked.nemo_action");
    let read = || nemo_files(&[data.path().to_path_buf()]);
    let now = Instant::now();
    let mut list = NemoList::default();
    std::fs::write(&file, &files("before")[1].1).unwrap();
    let first = list.request(read(), "", now).unwrap();
    list.finish(&first, first.to_vec(), now);
    assert_eq!(list.visible[0].name, "before");
    let later = now + Duration::from_secs(31);
    let previous = list.request(read(), "", later).unwrap();
    std::fs::write(&file, &files("after")[1].1).unwrap();
    let changed = list.request(read(), "", later).unwrap();
    assert!(list.visible.is_empty());
    list.finish(&previous, previous.to_vec(), later);
    assert!(list.visible.is_empty());
    list.finish(&changed, changed.to_vec(), later);
    assert_eq!(list.visible[0].name, "after");
}
