use super::*;
use flit_core::provider::{FileOp, Resolve};
use flit_platform::{FileOps, Job, Shell};
use std::time::Duration;

#[test]
fn recent_files_are_newest_first_unique_and_limited() {
    let root = crate::test_temp().join(format!("flit-recent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["links", "work"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for f in ["old.txt", "new.txt"] {
        std::fs::write(root.join(f), "x").unwrap();
    }
    let at = |rel: &str| FsPath::parse(root.join(rel).to_str().unwrap()).unwrap();
    let mut sh = crate::ShellCom::new().unwrap();
    let out = sh.file_op(&FileOp::Link { items: vec![at("old.txt"), at("work"), at("new.txt")], dest: at("links") }, &Job::detached(Resolve::Skip));
    let links: Vec<FsPath> = out.items.into_iter().map(|(_, l)| l).collect();
    let base = SystemTime::now() - Duration::from_secs(3600);
    for (k, l) in links.iter().enumerate() {
        std::fs::File::options().write(true).open(&*l.display()).unwrap().set_modified(base + Duration::from_secs(60 * k as u64)).unwrap();
    }
    std::fs::copy(&*links[0].display(), root.join(r"links\again.lnk")).unwrap();
    std::fs::File::options().write(true).open(root.join(r"links\again.lnk")).unwrap().set_modified(base - Duration::from_secs(60)).unwrap();
    std::fs::write(root.join(r"links\note.txt"), "not a link").unwrap();
    let got = recent_files(&root.join("links"), 8);
    let shown: Vec<(&str, bool)> = got.iter().map(|r| (r.name.as_str(), r.dir)).collect();
    assert_eq!(shown, [("new.txt", false), ("work", true), ("old.txt", false)]);
    assert_eq!(got[1].target, at("work"));
    assert!(got[0].at > got[1].at && got[1].at > got[2].at);
    assert_eq!(recent_files(&root.join("links"), 1).len(), 1);
    assert!(recent_files(&root.join("missing"), 8).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

fn namespace_link(target: &str, link: &Path) {
    use windows::Win32::UI::Shell::SHParseDisplayName;
    let name = wide(target);
    let file = wide(link.to_str().unwrap());
    unsafe {
        let mut pidl = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None).unwrap();
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        sl.SetIDList(pidl).unwrap();
        ILFree(Some(pidl));
        sl.cast::<IPersistFile>().unwrap().Save(PCWSTR(file.as_ptr()), true).unwrap();
    }
}

#[test]
fn a_recent_this_pc_link_is_the_computer_folder_and_other_namespace_links_are_skipped() {
    crate::com_sta();
    let root = crate::test_temp().join(format!("flit-recent-pc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    namespace_link(flit_core::path::SHELL_COMPUTER, &root.join("This PC.lnk"));
    namespace_link("shell:::{26EE0668-A00A-44D7-9371-BEB064C98683}", &root.join("Control Panel.lnk"));
    let got = recent_files(&root, 8);
    let shown: Vec<(FsPath, bool)> = got.iter().map(|r| (r.target.clone(), r.dir)).collect();
    assert_eq!(shown, [(FsPath::computer(), true)], "This PC opens as a folder in Flit, never through the shell as a file");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn business_onedrive_accounts_require_an_existing_folder_and_use_its_name_as_fallback() {
    let root = crate::test_temp().join(format!("flit-business-onedrive-{}", std::process::id()));
    let folder = root.join("OneDrive - Завод");
    std::fs::create_dir_all(&folder).unwrap();
    let file = root.join("file");
    std::fs::write(&file, "").unwrap();
    let account = |key: &str, path: Option<&Path>, name: Option<&str>| business_account(key, |field| match field {
        "UserFolder" => path.map(|p| p.to_str().unwrap().to_owned()),
        "DisplayName" => name.map(str::to_owned),
        _ => panic!("unexpected field {field}"),
    });
    let location = Location::from_local(FsPath::parse(folder.to_str().unwrap()).unwrap());
    assert_eq!(account("Business12", Some(&folder), Some(" Example ")), Some((12, ("Example".into(), location.clone()))));
    assert_eq!(account("business2", Some(&folder), None), Some((2, ("OneDrive - Завод".into(), location))));
    for key in ["Personal", "Business", "BusinessOld", "Business+1"] {
        assert!(account(key, Some(&folder), None).is_none());
    }
    for path in [None, Some(root.join("absent").as_path()), Some(file.as_path())] {
        assert!(account("Business1", path, None).is_none());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn maps_windows_ui_languages_to_flit_codes() {
    assert_eq!([0x0419, 0x0409, 0x0809, 0x0407, 0x040C, 0x0C0A, 0x0422, 0x0804, 0x1004].map(language_code), ["ru", "en", "en", "de", "fr", "es", "uk", "zh-CN", "zh-CN"]);
    assert_eq!([0x0404, 0x0C04, 0x0411].map(language_code), ["", "", ""]);
}

#[test]
fn drives_end_with_the_network_place_and_mapped_drives_carry_their_share() {
    let all = drives();
    assert_eq!(all.last().map(|d| d.path.clone()), Some(FsPath::network()));
    assert!(all.iter().filter(|d| d.remote.is_some()).all(|d| d.path.kind() == flit_core::path::PathKind::Drive && d.label.contains('\\')));
}

#[test]
fn a_text_document_is_offered_even_when_the_registry_has_no_template_for_it() {
    let t = |label: &str, ext: &str| NewTemplate { label: label.into(), ext: ext.into(), stem: format!("New {label}"), template: Template::Empty, names: None };
    let added = with_text_document(vec![t("Microsoft Word Document", ".docx"), t("Bitmap image", ".bmp")]);
    let labels: Vec<&str> = added.iter().map(|t| t.label()).collect();
    assert_eq!(labels, ["Bitmap image", "Microsoft Word Document", S::MenuTextDocument.t()]);
    let text = added.iter().find(|t| t.ext == ".txt").unwrap();
    assert!(text.names.is_some() && text.template == Template::Empty, "it follows the language and starts empty");
    let kept = with_text_document(vec![t("Notepad text", ".TXT")]);
    assert_eq!(kept.len(), 1, "a registered text template is not doubled");
    assert_eq!(kept[0].label, "Notepad text");
}
