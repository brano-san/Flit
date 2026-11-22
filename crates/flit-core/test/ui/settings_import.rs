use super::*;
use crate::ui::Persisted;
use std::time::Instant;

fn app() -> App {
    App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (380, 320), Instant::now())
}

fn sample() -> Doc {
    settings::parse(r##"{"files.show_hidden":true,"menu.custom":[{"name":"View logs","command":"viewer.exe %f"}],"tags":[{"name":"Archive","color":"#8899AA"}]}"##)
}

fn confirmed_plan(a: &mut App) -> ImportPlan {
    a.form_submit(0);
    a.drain_effects().find_map(|e| if let Effect::Settings(SettingsCmd::ConfirmImport(plan)) = e { Some(*plan) } else { None }).expect("approval requests a backup")
}

#[test]
fn the_preview_lists_programs_and_changes_then_applies_and_undoes_after_backup() {
    let mut a = app();
    let baseline = a.setting_values();
    a.preview_settings_import(sample());
    let form = a.dialogs.form.as_ref().unwrap();
    assert_eq!(form.focus, form.controls.len() + 1);
    assert!(form.controls.iter().any(|c| matches!(c, Control::Paragraph { text, .. } if text.contains("menu.custom"))));
    assert!(form.controls.iter().any(|c| matches!(c, Control::Paragraph { text, .. } if text.contains("viewer.exe"))));
    assert!(form.controls.iter().any(|c| matches!(c, Control::Line(key) if key == "files.show_hidden")));
    assert_eq!(a.setting_values(), baseline);
    let plan = confirmed_plan(&mut a);
    let backup = settings::parse(&plan.backup(&Default::default()));
    assert_eq!(backup.value("files.show_hidden"), Some(&Value::Bool(false)));
    assert_eq!(a.setting_values(), baseline);
    a.apply_confirmed_settings_import(plan, "settings.before-import.json".into());
    assert_eq!(SETTINGS[settings::index("files.show_hidden").unwrap()].get(&a), Value::Bool(true));
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Tags(TagRequest::ChangeImport { after, .. }) if after.iter().any(|tag| tag.name == "Archive"))));
    a.form_submit(0);
    assert_eq!(a.setting_values(), baseline);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Tags(TagRequest::ChangeImport { before, after }) if before.len() == after.len() + 1)));
}

#[test]
fn cancelling_or_changing_settings_before_the_backup_reply_prevents_the_import() {
    let mut a = app();
    let baseline = a.setting_values();
    a.preview_settings_import(sample());
    a.form_submit(1);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::ConfirmImport(_)))));
    assert_eq!(a.setting_values(), baseline);

    a.preview_settings_import(sample());
    let plan = confirmed_plan(&mut a);
    let key = settings::index("files.show_hidden").unwrap();
    a.apply_settings(&[(key, Value::Bool(true))]);
    a.apply_confirmed_settings_import(plan, String::new());
    assert!(a.dialogs.form.is_none());
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::Tags(_))));
    assert_eq!(SETTINGS[settings::index("menu.custom").unwrap()].get(&a), baseline[settings::index("menu.custom").unwrap()]);
}
