use super::*;
use crate::temp::Temp;
use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

fn script(t: &Temp, name: &str, body: &str) -> PathBuf {
    let p = t.path().join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn engine(kind: Kind, program: PathBuf) -> Engine {
    Engine { kind, program, lead: &[] }
}

fn collect(engines: &[Engine], folder: Option<&Path>, query: &str) -> Result<Vec<(String, u64)>, PlatformError> {
    let mut out = Vec::new();
    search_with(engines, folder, query, &|| true, &mut |path, meta| out.push((String::from_utf8_lossy(path.as_bytes()).into_owned(), meta.size)))?;
    out.sort();
    Ok(out)
}

fn os(args: Vec<OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

#[test]
fn the_desktop_s_own_index_is_asked_first() {
    let all = |p: &str| Some(PathBuf::from(format!("/usr/bin/{p}")));
    let kinds = |desktop: &str, find: &dyn Fn(&str) -> Option<PathBuf>| engines(desktop, find).into_iter().map(|e| (e.kind, e.program.file_name().unwrap().to_string_lossy().into_owned())).collect::<Vec<_>>();
    assert_eq!(kinds("KDE", &all), [(Kind::Baloo, "baloosearch6".to_owned()), (Kind::Recoll, "recollq".to_owned()), (Kind::Tracker, "localsearch".to_owned())]);
    assert_eq!(kinds("ubuntu:GNOME", &all), [(Kind::Tracker, "localsearch".to_owned()), (Kind::Recoll, "recollq".to_owned()), (Kind::Baloo, "baloosearch6".to_owned())]);
    let older = |p: &str| ["tracker3", "baloosearch", "recoll"].contains(&p).then(|| PathBuf::from(format!("/usr/bin/{p}")));
    let found = engines("XFCE", &older);
    assert_eq!(found.iter().map(|e| e.program.file_name().unwrap().to_str().unwrap()).collect::<Vec<_>>(), ["tracker3", "recoll", "baloosearch"]);
    assert_eq!(found[1].lead, ["-t"]);
    assert_eq!(found[0].lead, ["search"]);
    assert!(engines("KDE", &|_| None).is_empty());
}

#[test]
fn each_engine_gets_its_own_arguments_and_terms_never_become_options() {
    let terms = terms("  -draft \"annual report\"\tq3 ");
    assert_eq!(terms, ["-draft", "\"annual report\"", "q3"]);
    let tracker = Engine { kind: Kind::Tracker, program: "/x".into(), lead: &["search"] };
    assert_eq!(os(tracker.args(Some(Path::new("/home/me")), &terms)), ["search", "--disable-color", "--disable-snippets", "--limit", LIMIT, "--", "-draft", "annual report", "q3"]);
    let baloo = engine(Kind::Baloo, "/x".into());
    assert_eq!(os(baloo.args(Some(Path::new("/home/me/My Docs")), &terms)), ["--limit", LIMIT, "--directory", "/home/me/My Docs", "--", "-draft", "\"annual report\"", "q3"]);
    assert_eq!(os(baloo.args(None, &terms[2..])), ["--limit", LIMIT, "--", "q3"]);
    let recoll = Engine { kind: Kind::Recoll, program: "/x".into(), lead: &["-t"] };
    assert_eq!(os(recoll.args(None, &terms)), ["-t", "-b", "-n", LIMIT, " -draft \"annual report\" q3"]);
    assert_eq!(os(recoll.args(None, &terms[2..])), ["-t", "-b", "-n", LIMIT, "q3"]);
}

#[test]
fn output_of_tracker_baloo_and_recoll_becomes_paths() {
    let tracker = b"\x1b[1mFiles:\x1b[0m\n  \x1b[32mfile:///home/me/My%20Docs/a.txt\x1b[0m\n  file://localhost/home/me/b.txt\n\nUse --limit to see more\n";
    let baloo = b"/home/me/notes with space.md\nElapsed: 1.52 msecs\n";
    let recoll = b"file:///home/me/c%C3%A9.odt\nRecoll query: (x)\n2 results\nfile://relative/no\n";
    let paths: Vec<PathBuf> = [&tracker[..], &baloo[..], &recoll[..]].iter().flat_map(|out| out.split(|&b| b == b'\n')).filter_map(parse_line).collect();
    assert_eq!(paths, ["/home/me/My Docs/a.txt", "/home/me/b.txt", "/home/me/notes with space.md", "/home/me/cé.odt"].map(PathBuf::from));
}

#[test]
fn results_are_files_inside_the_searched_folder_that_still_exist() {
    let t = Temp::new("content-folder");
    let inside = t.file("in/report.txt", 5);
    let outside = t.file("out/report.txt", 7);
    let twin = t.file("in-sibling/report.txt", 9);
    let gone = t.path().join("in/deleted.txt");
    let fake = script(&t, "baloosearch6", &format!("echo {}\necho {}\necho {}\necho {}\necho {}\necho 'Elapsed: 1 msecs'", inside.display(), outside.display(), twin.display(), gone.display(), inside.display()));
    let engines = [engine(Kind::Baloo, fake)];
    assert_eq!(collect(&engines, Some(&t.path().join("in")), "report").unwrap(), [(inside.display().to_string(), 5)]);
    assert_eq!(collect(&engines, None, "report").unwrap().len(), 3);
}

#[test]
fn a_broken_engine_hands_over_to_the_next_and_its_error_is_shown_when_none_works() {
    let t = Temp::new("content-fallback");
    let file = t.file("doc.txt", 3);
    let broken = script(&t, "tracker3", "echo 'Could not establish a connection to Tracker' >&2\nexit 1");
    let working = script(&t, "recollq", &format!("echo file://{}", file.display()));
    let both = [Engine { kind: Kind::Tracker, program: broken.clone(), lead: &["search"] }, engine(Kind::Recoll, working)];
    assert_eq!(collect(&both, None, "doc").unwrap(), [(file.display().to_string(), 3)]);
    let only = [Engine { kind: Kind::Tracker, program: broken, lead: &["search"] }];
    let e = collect(&only, None, "doc").unwrap_err();
    assert_eq!(e.message, tr_en!(S::ErrorsEngineContentSearchFailed, program = "tracker3", error = "Could not establish a connection to Tracker"));
}

#[test]
fn without_an_engine_or_words_nothing_is_run() {
    let e = collect(&[], None, "doc").unwrap_err();
    assert_eq!(e.message, crate::shell::tool_missing("recollq").message);
    let t = Temp::new("content-empty");
    let marker = t.path().join("ran");
    let fake = script(&t, "recollq", &format!("touch {}\nexit 1", marker.display()));
    assert!(collect(&[engine(Kind::Recoll, fake)], None, "   ").unwrap().is_empty());
    assert!(!marker.exists());
}

#[test]
fn missing_content_search_helpers_name_a_package_and_any_installed_engine_still_runs() {
    crate::temp::with_tools(&[], || {
        let error = search(None, "doc", &|| true, &mut |_, _| {}).unwrap_err();
        crate::temp::assert_missing(&error.message, "recollq");
    });
    for tool in ["recollq", "recoll", "localsearch", "tracker3", "baloosearch6", "baloosearch"] {
        crate::temp::with_tools(&[tool], || {
            search(None, "doc", &|| true, &mut |_, _| {}).unwrap();
        });
    }
}

#[test]
fn closing_the_search_stops_a_slow_engine() {
    let t = Temp::new("content-cancel");
    let file = t.file("first.txt", 1);
    let slow = script(&t, "baloosearch", &format!("echo {}\nexec sleep 30", file.display()));
    let started = Instant::now();
    let mut seen = 0;
    let calls = std::cell::Cell::new(0);
    let r = search_with(
        &[engine(Kind::Baloo, slow)],
        None,
        "first",
        &|| {
            calls.set(calls.get() + 1);
            calls.get() < 5
        },
        &mut |_, _| seen += 1,
    );
    assert!(r.is_ok());
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(seen, 1);
}
