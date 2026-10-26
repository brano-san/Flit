use super::*;

#[test]
fn modules_include_this_process_and_system_dlls_are_not_third_party() {
    let all = modules();
    let exe = std::env::current_exe().unwrap();
    assert!(all.contains(&exe));
    assert!(all.iter().any(|m| m.to_string_lossy().to_lowercase().ends_with("kernel32.dll")));
    let foreign = third_party(&all);
    assert!(!foreign.contains(&exe));
    assert!(!foreign.iter().any(|m| m.to_string_lossy().to_lowercase().ends_with("kernel32.dll")));
}

#[test]
fn gpus_list_at_least_the_software_adapter() {
    assert!(!gpus().is_empty());
}
