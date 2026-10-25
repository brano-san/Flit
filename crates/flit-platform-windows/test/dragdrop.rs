use super::*;

fn text(units: [u16; 260]) -> String {
    String::from_utf16_lossy(&units[..units.iter().position(|&c| c == 0).unwrap_or(units.len())])
}

fn read_description(data: &IDataObject) -> (DROPIMAGETYPE, String, String) {
    let d = unsafe {
        let mut medium = data.GetData(&description_format()).unwrap();
        let bytes = global_bytes(medium.u.hGlobal).unwrap();
        ReleaseStgMedium(&mut medium);
        assert!(bytes.len() >= size_of::<DROPDESCRIPTION>());
        std::ptr::read_unaligned(bytes.as_ptr() as *const DROPDESCRIPTION)
    };
    let (kind, message, insert) = (d.r#type, d.szMessage, d.szInsert);
    (kind, text(message), text(insert))
}

#[test]
fn drop_captions_travel_in_the_dragged_data_object() {
    crate::com_sta();
    let dir = std::env::temp_dir().join(format!("flit-dropdesc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let file = FsPath::parse(dir.join("a.txt").to_str().unwrap()).unwrap();
    let data: IDataObject = Pidls::parse(&[file]).unwrap().ui_object(HWND::default()).unwrap();
    describe(&data, Some(&(DROPEFFECT_MOVE.0, "Move to %1".into(), "Documents".into())));
    assert_eq!(read_description(&data), (DROPIMAGETYPE(DROPEFFECT_MOVE.0 as i32), "Move to %1".to_owned(), "Documents".to_owned()));
    describe(&data, Some(&(DROPEFFECT_COPY.0, "Copy to %1".into(), "x".repeat(400))));
    assert_eq!(read_description(&data).2.len(), 259, "long names are cut to fit");
    describe(&data, None);
    assert_eq!(read_description(&data).0, DROPIMAGE_INVALID);
    let _ = std::fs::remove_dir_all(&dir);
}
