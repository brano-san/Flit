use super::*;

#[test]
fn tail_keeps_the_last_lines() {
    assert_eq!(tail("a\nb\nc\nd\n", 2), "c\nd\n");
    assert_eq!(tail("a\nb", 5), "a\nb");
    assert_eq!(tail("", 3), "");
}

#[test]
fn the_report_names_version_os_paths_and_modules() {
    let r = report::<crate::P>();
    assert!(r.starts_with(&format!("Flit {}", env!("CARGO_PKG_VERSION"))), "{r}");
    assert!(r.contains("\nOS: ") && r.contains("\nSettings: ") && r.contains("\nLogs: "), "{r}");
    #[cfg(windows)]
    assert!(r.contains("\nGPU:\n  ") && r.contains("Modules loaded in process"), "{r}");
}
