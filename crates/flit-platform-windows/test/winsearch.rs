use super::*;

#[test]
fn search_urls_escape_the_query_and_the_folder() {
    assert_eq!(url(None, "kind:music"), "search-ms:query=kind%3Amusic&");
    let folder = FsPath::parse(r"C:\Users\Jo Doe").unwrap();
    assert_eq!(url(Some(&folder), "author:\"Ann\""), "search-ms:query=author%3A%22Ann%22&crumb=location:C%3A%5CUsers%5CJo%20Doe&");
}

#[test]
#[ignore = "needs the Windows Search service and an indexed Windows folder"]
fn windows_search_finds_system_files() {
    let folder = FsPath::parse(r"C:\Windows").unwrap();
    let mut hits = Vec::new();
    let started = std::time::Instant::now();
    search(Some(&folder), "kind:program", &|| true, &mut |p, m| hits.push((p, m))).unwrap();
    println!("{} results in {:.1} ms", hits.len(), started.elapsed().as_secs_f64() * 1e3);
}
