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
    assert_eq!((hint_style("'full'\n"), hint_style("hintmedium")), (Some(Hinting::Full), Some(Hinting::Full)));
    assert_eq!((hint_style("'slight'"), hint_style("'none'"), hint_style("")), (Some(Hinting::Slight), Some(Hinting::None), None));
}

#[test]
fn an_unknown_desktop_font_falls_back_to_the_fontconfig_default() {
    assert_eq!(gnome_font_family("''"), None);
    assert_eq!(kde_font_family("[General]\nfixed=Hack,10\n"), None);
    let Some(fc) = Fontconfig::load() else { return };
    assert_eq!(fc.resolve(None, Some("No Such Font Family 4242")).ui, fc.resolve(None, None).ui);
}

fn merged(desktop: Prefs, fontconfig: Prefs) -> (Subpixel, Hinting) {
    let r = Rendering::merge(desktop, fontconfig);
    (r.subpixel, r.hinting)
}

#[test]
fn the_subpixel_order_and_hinting_come_from_gnome_kde_or_fontconfig() {
    let none = Prefs::default();
    assert_eq!(merged(gnome_prefs(Some("'rgba'"), Some("'bgr'"), Some("'slight'")), none), (Subpixel::Bgr, Hinting::Slight));
    assert_eq!(merged(gnome_prefs(Some("rgba"), None, Some("full")), none), (Subpixel::Rgb, Hinting::Full), "GNOME's default order is rgb");
    assert_eq!(merged(gnome_prefs(Some("'rgba'"), Some("'vrgb'"), None), none), (Subpixel::Vrgb, Hinting::None));
    let kde = kde_prefs("[General]\nXftAntialias=true\nXftHintStyle=hintslight\nXftSubPixel=vbgr\n");
    assert_eq!(merged(kde, none), (Subpixel::Vbgr, Hinting::Slight));
    let fc = fontconfig_prefs(Some(true), Some(1), Some(true), Some(1));
    assert_eq!(merged(none, fc), (Subpixel::Rgb, Hinting::Slight));
    assert_eq!(merged(kde_prefs("[General]\nfont=Noto Sans,10\n"), fc), (Subpixel::Rgb, Hinting::Slight), "KDE without Xft keys follows fontconfig");
    assert_eq!(merged(none, fontconfig_prefs(None, Some(2), None, Some(3))), (Subpixel::Bgr, Hinting::Full));
    for r in [Rendering { hinting: Hinting::Slight, subpixel: Subpixel::Vbgr }, Rendering::default(), Rendering { hinting: Hinting::Full, subpixel: Subpixel::Bgr }] {
        assert_eq!(Rendering::unpack(r.pack()), r);
    }
}

#[test]
fn grayscale_or_no_antialiasing_keeps_plain_text_whatever_fontconfig_says() {
    let fc = fontconfig_prefs(Some(true), Some(1), Some(true), Some(3));
    assert_eq!(merged(gnome_prefs(Some("'grayscale'"), Some("'rgb'"), None), fc), (Subpixel::None, Hinting::Full), "the desktop's grayscale wins over fontconfig's rgb");
    assert_eq!(merged(gnome_prefs(Some("'none'"), Some("'rgb'"), None), fc).0, Subpixel::None);
    assert_eq!(merged(kde_prefs("[General]\nXftAntialias=false\nXftSubPixel=rgb\n"), Prefs::default()).0, Subpixel::None);
    assert_eq!(merged(kde_prefs("[General]\nXftSubPixel=none\n"), fc).0, Subpixel::None);
    assert_eq!(merged(Prefs::default(), fontconfig_prefs(Some(false), Some(1), None, None)).0, Subpixel::None);
    assert_eq!(merged(Prefs::default(), fontconfig_prefs(Some(true), Some(0), Some(false), Some(3))), (Subpixel::None, Hinting::None), "unknown rgba and hinting=false");
    assert_eq!(merged(Prefs::default(), Prefs::default()), (Subpixel::None, Hinting::None), "nothing known is unhinted plain text like before");
}

#[test]
fn a_changed_gnome_key_is_combined_with_the_other_two() {
    let mut raw: [Option<String>; 3] = [Some("'grayscale'".into()), Some("'bgr'".into()), Some("'slight'".into())];
    assert_eq!(merged(gnome_raw_prefs(&raw), Prefs::default()), (Subpixel::None, Hinting::Slight));
    assert!(gnome_raw_update(&mut raw, "font-antialiasing", "rgba"));
    assert_eq!(merged(gnome_raw_prefs(&raw), Prefs::default()), (Subpixel::Bgr, Hinting::Slight), "switching to rgba uses the order chosen before");
    assert!(gnome_raw_update(&mut raw, "font-rgba-order", "rgb"));
    assert!(gnome_raw_update(&mut raw, "font-hinting", "full"));
    assert_eq!(merged(gnome_raw_prefs(&raw), Prefs::default()), (Subpixel::Rgb, Hinting::Full));
    assert!(!gnome_raw_update(&mut raw, "font-name", "Cantarell 11"));
    assert_eq!(raw[1].as_deref(), Some("rgb"));
}

#[test]
fn fontconfig_reports_its_rendering_preferences() {
    let Some(fc) = Fontconfig::load() else { return };
    let p = fc.prefs(None);
    assert_ne!(p.antialias, Some(false), "fontconfig antialiases outline fonts by default: {p:?}");
}

fn gnome_defaults(key: &str) -> Option<String> {
    match key {
        "font-name" => Some("'Cantarell 11'".into()),
        "font-antialiasing" => Some("'grayscale'".into()),
        "font-rgba-order" => Some("'rgb'".into()),
        "font-hinting" => Some("'slight'".into()),
        _ => None,
    }
}

#[test]
fn desktops_without_gnome_settings_follow_fontconfig_even_when_gsettings_answers() {
    let fc = fontconfig_prefs(Some(true), Some(1), Some(true), Some(3));
    for name in ["XFCE", "MATE", "X-Cinnamon", "sway", "LXQt", ""] {
        assert_eq!(source(name), Source::Fontconfig, "{name}");
        let d = desktop_from(source(name), String::new, gnome_defaults, &mut Default::default());
        assert_eq!(d.family, None, "{name}");
        assert_eq!(merged(d.prefs, fc), (Subpixel::Rgb, Hinting::Full), "{name}: fontconfig's rgba and hintfull win");
    }
}

#[test]
fn gnome_like_desktops_still_read_gsettings_and_kde_reads_kdeglobals() {
    let fc = fontconfig_prefs(Some(true), Some(1), Some(true), Some(3));
    for name in ["GNOME", "ubuntu:GNOME", "Unity", "Budgie:GNOME", "Pantheon"] {
        assert_eq!(source(name), Source::Gnome, "{name}");
        let d = desktop_from(source(name), String::new, gnome_defaults, &mut Default::default());
        assert_eq!(d.family.as_deref(), Some("Cantarell"));
        assert_eq!(merged(d.prefs, fc), (Subpixel::None, Hinting::Slight), "{name}: the desktop's grayscale and slight win");
    }
    assert_eq!(source("KDE"), Source::Kde);
    let d = desktop_from(Source::Kde, || "[General]\nXftSubPixel=bgr\n".into(), gnome_defaults, &mut Default::default());
    assert_eq!(merged(d.prefs, fc).0, Subpixel::Bgr);
}
