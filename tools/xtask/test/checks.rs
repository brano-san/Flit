use crate::{colors, layers};
use serde_json::json;

fn metadata(bad_dependency: bool) -> layers::Metadata {
    let names = ["flit-core", "flit-gen", "xtask", "flit-proto", "flit-sign", "flit-search", "flit-platform", "flit-archive", "flit-platform-linux", "flit-platform-windows", "flit-platform-linux-gui", "flit", "flit-indexd"];
    let mut members = names.to_vec();
    if bad_dependency {
        members.push("flit-unregistered");
    }
    let packages: Vec<_> = members.iter().map(|&name| {
        let dependency = match (name, bad_dependency) {
            ("flit-proto", false) => Some("flit-core"),
            ("flit-core", true) => Some("flit-proto"),
            _ => None,
        };
        let deps: Vec<_> = dependency.into_iter().map(|dep| json!({"name":dep, "rename":"alias", "path":format!("/repo/{dep}")})).collect();
        json!({"id":name, "name":name, "manifest_path":format!("/repo/{name}/Cargo.toml"), "dependencies":deps})
    }).collect();
    serde_json::from_value(json!({"packages":packages, "workspace_members":members, "resolve":{"nodes":[]}})).unwrap()
}

#[test]
fn workspace_accepts_lower_layers_and_rejects_higher_and_unknown_crates() {
    let mut problems = Vec::new();
    layers::check(&metadata(false), &mut problems);
    assert!(problems.is_empty(), "{problems:?}");
    layers::check(&metadata(true), &mut problems);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("flit-core (layer 0) depends on flit-proto (layer 1)")));
    assert!(problems.iter().any(|p| p.contains("Unregistered workspace crate: flit-unregistered")));
}

#[test]
fn color_check_distinguishes_theme_literals_and_protocol_constants() {
    let source = r##"
const MASK: u32 = 0xFF00_0000;
fn draw(user_color: u32, dark: bool) {
    let _ = Color::rgb(user_color & 0xFF_FFFF);
    let _ = Color(user_color | 0xFF00_0000);
    let _ = Editor::Color(0);
    let _ = "Color::rgb(0x123456)";
    let _ = render::Color::rgba(
        0x0011_22FF,
    );
    let _ = Color::rgb(if dark { 0 } else { user_color });
    let _ = "#ff00ff";
    let _ = Color(255);
}
const BADGE_COLOR: [u8; 4] = [3, 4, 5, 255];
fn drag(user_color: u32) {
    let _ = WindowAux::new().background_pixel(0xf4f4f4);
    let _ = vec![Ink::rgb(0xffffff), Ink(user_color)];
}
use flit_core::render::{Color as Ink, Rect};
"##;
    assert_eq!(colors::lines(source).unwrap(), [8, 11, 12, 13, 15, 17, 18]);
}
