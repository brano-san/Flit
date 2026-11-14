use super::*;

fn programs(path: &str, desktop: &str) -> Vec<Vec<String>> {
    attempts(Path::new(path), desktop).into_iter().map(|a| a.into_iter().map(|c| c[..4.min(c.len())].join(" ")).collect()).collect()
}

#[test]
fn each_desktop_gets_its_own_way_to_set_the_background_and_the_portal_last() {
    assert_eq!(programs("/p/a b.png", "KDE")[0], ["plasma-apply-wallpaperimage /p/a b.png"]);
    assert_eq!(programs("/p/a.png", "ubuntu:GNOME")[0], ["gsettings set org.gnome.desktop.background picture-uri", "gsettings set org.gnome.desktop.background picture-uri-dark"]);
    assert_eq!(programs("/p/a.png", "X-Cinnamon")[0], ["gsettings set org.cinnamon.desktop.background picture-uri"]);
    assert_eq!(programs("/p/a.png", "MATE")[0], ["gsettings set org.mate.background picture-filename"]);
    for desktop in ["KDE", "GNOME", "XFCE", ""] {
        let all = attempts(Path::new("/p/a b.png"), desktop);
        let portal = all.last().unwrap();
        assert_eq!(portal[0][8], "org.freedesktop.portal.Wallpaper.SetWallpaperURI", "{desktop}");
        assert_eq!(portal[0][10], "file:///p/a%20b.png");
    }
}

#[test]
fn an_unknown_desktop_only_asks_the_portal() {
    assert_eq!(attempts(Path::new("/p/a.png"), "XFCE").len(), 1);
    assert_eq!(attempts(Path::new("/p/a.png"), "").len(), 1);
}
