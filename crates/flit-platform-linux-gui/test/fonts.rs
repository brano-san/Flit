use super::*;

fn file(path: &[u8], index: u32) -> FontFile {
    FontFile { path: PathBuf::from(std::ffi::OsStr::from_bytes(path)), index }
}

#[test]
fn the_cache_round_trips_and_rejects_foreign_files() {
    let pair = |a: &[u8], b: &[u8]| [file(a, 0), file(b, 65537)];
    let r = Resolved {
        ui: Some(pair(b"/usr/share/fonts/a b/Sans.ttf", b"/usr/share/fonts/Sans\tBold.ttf")),
        mono: Some(pair(b"/m.ttf", b"/m.ttf")),
        fallback: vec![file(b"/f/cjk.ttc", 2), file(b"/f/\xff\xfe.ttf", 0)],
        family: Some(("Ubuntu Light".into(), pair(b"/u.ttf", b"/ub.ttf"))),
    };
    assert_eq!(Resolved::decode(&r.encode()), Some(r.clone()));
    assert_eq!(Resolved::decode(b"something else\nui\t0\t/x\n"), None);
    let none = Resolved { family: None, ..r };
    assert_eq!(Resolved::decode(&none.encode()), Some(none));
    let dir = std::env::temp_dir().join(format!("flit-fonts-cache-{}", std::process::id()));
    let path = dir.join("flit").join("fonts");
    save_cache(&path, &Resolved::default());
    assert_eq!(load_cache(&path), Some(Resolved::default()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fontconfig_resolves_the_defaults_a_fallback_chain_and_the_family_list() {
    let fc = Fontconfig::load().expect("libfontconfig.so.1");
    let r = fc.resolve(Some("DejaVu Sans Mono"), None);
    let [ui, ui_bold] = r.ui.clone().unwrap();
    assert!(ui.path.exists() && ui_bold.path.exists(), "{r:?}");
    assert!(r.mono.is_some());
    assert_eq!(r.family.as_ref().unwrap().0, "DejaVu Sans Mono");
    assert!(r.fallback.len() > 1, "{r:?}");
    assert!(r.fallback.iter().all(|f| f.path.exists()));
    assert!(fc.family("No Such Font Family 42").is_none());
    let families = fc.families();
    assert!(families.iter().any(|f| f == "DejaVu Sans") && families.iter().any(|f| f == "DejaVu Sans Mono"), "{families:?}");
    assert!(families.windows(2).all(|w| w[0].to_lowercase() < w[1].to_lowercase()));
}

#[test]
fn mapped_files_are_shared_and_missing_files_are_refused() {
    let a = map_file(Path::new("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")).unwrap();
    let b = map_file(Path::new("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")).unwrap();
    assert_eq!(a.as_ptr(), b.as_ptr());
    assert_eq!(&a[..4], &[0, 1, 0, 0]);
    assert!(map_file(Path::new("/nonexistent/flit.ttf")).is_none());
}

#[test]
#[ignore]
fn bench_fontconfig() {
    let t = Instant::now();
    let fc = Fontconfig::load().unwrap();
    let first = t.elapsed();
    let warm: Vec<Duration> = (0..5)
        .map(|_| {
            let t = Instant::now();
            drop(Fontconfig::load().unwrap());
            t.elapsed()
        })
        .collect();
    let t = Instant::now();
    let r = fc.resolve(None, None);
    let resolve = t.elapsed();
    let t = Instant::now();
    let families = fc.families();
    let list = t.elapsed();
    let path = std::env::temp_dir().join(format!("flit-fonts-bench-{}", std::process::id())).join("fonts");
    save_cache(&path, &r);
    let t = Instant::now();
    let cached = load_cache(&path).unwrap();
    let read = t.elapsed();
    assert_eq!(cached, r);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    println!(
        "fontconfig init first {first:?}, again {warm:?}; resolve (sans, mono, {} fallbacks) {resolve:?}; {} families {list:?}; cache read {read:?} ({} bytes)",
        r.fallback.len(),
        families.len(),
        r.encode().len()
    );
}

#[test]
fn the_desktop_font_and_hinting_are_read_from_gnome_and_kde_settings() {
    assert_eq!(gnome_font_family("'Cantarell 11'\n").as_deref(), Some("Cantarell"));
    assert_eq!(gnome_font_family("'Noto Sans CJK JP 10.5'").as_deref(), Some("Noto Sans CJK JP"));
    assert_eq!(gnome_font_family("'Inter'").as_deref(), Some("Inter"));
    assert_eq!(kde_font_family("[General]\nfixed=Hack,10,-1,5,50,0,0,0,0,0\nfont=Noto Sans,10,-1,5,50,0,0,0,0,0\n").as_deref(), Some("Noto Sans"));
    assert!(full_hinting("'full'\n") && full_hinting("hintmedium"));
    assert!(!full_hinting("'slight'") && !full_hinting("'none'") && !full_hinting(""), "slight hinting stays unhinted like before");
}

#[test]
fn an_unknown_desktop_font_falls_back_to_the_fontconfig_default() {
    assert_eq!(gnome_font_family("''"), None);
    assert_eq!(kde_font_family("[General]\nfixed=Hack,10\n"), None);
    let Some(fc) = Fontconfig::load() else { return };
    assert_eq!(fc.resolve(None, Some("No Such Font Family 4242")).ui, fc.resolve(None, None).ui);
}
