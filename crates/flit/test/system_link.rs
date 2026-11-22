use super::*;

#[test]
fn flit_link_opens_a_local_folder_or_names_an_existing_search() {
    assert_eq!(
        parse("FLIT:open?path=C%3A%2FMy%20Folder", true, &|path| path == Path::new("C:/My Folder")),
        FsPath::parse(r"C:\My Folder").map(Link::Folder)
    );
    assert_eq!(
        parse("flit://open?path=%2Fhome%2FDocuments", false, &|path| path == Path::new("/home/Documents")),
        FsPath::parse("/home/Documents").map(Link::Folder)
    );
    assert_eq!(
        parse("flit:search?q=quarterly+reports", false, &|_| panic!("a saved search has no file path")),
        Some(Link::SavedSearch("quarterly reports".into()))
    );
}

#[test]
fn flit_link_refuses_files_network_paths_traversal_and_bad_encoding() {
    for (windows, uri) in [
        (false, "flit:open?path=/tmp/archive.zip"),
        (true, "flit:open?path=C:/archive.zip"),
        (true, "flit:open?path=%5C%5Cserver%5Cshare"),
        (true, "flit:open?path=//server/share"),
        (false, "flit:open?path=//server/share"),
        (false, "flit:open?path=/home/%2e%2e/etc"),
        (true, "flit:open?path=C:/folder/..%20/other"),
        (true, "flit:open?path=C:/folder:stream"),
        (false, "flit:open?path="),
        (false, "flit:search?q="),
        (false, "flit:search?q=search&path=/tmp"),
        (false, "flit:open?path=/tmp/%Q"),
        (false, "flit:open?path=/tmp/%FF"),
        (false, "flit:open?path=/tmp/%00"),
        (false, "flit:run?path=/tmp"),
    ] {
        assert_eq!(parse(uri, windows, &|path| !path.ends_with("archive.zip")), None, "{uri}");
    }
}

#[test]
fn flit_link_invocation_does_not_forward_extra_command_line_arguments() {
    for args in [
        vec![HANDLER, "flit:search?q=test", "--unregister"],
        vec!["--install-update", "flit:search?q=test"],
        vec![HANDLER, "--unregister"],
    ] {
        let args: Vec<_> = args.into_iter().map(OsString::from).collect();
        assert_eq!(cli(&args), Some(Err(S::SettingsPageSystemLinkInvalid)));
    }
    assert!(cli(&[OsString::from("archive.zip")]).is_none());
}

#[test]
fn flit_link_preserves_plus_in_paths_and_decodes_form_spacing_in_searches() {
    for raw in ["/home/u/a+b", "/home/u/a%2Bb"] {
        assert_eq!(
            parse(&format!("flit:open?path={raw}"), false, &|path| path == Path::new("/home/u/a+b")),
            FsPath::parse("/home/u/a+b").map(Link::Folder)
        );
    }
    assert_eq!(
        parse("flit:search?q=a+b%2Bc", false, &|_| unreachable!()),
        Some(Link::SavedSearch("a b+c".into()))
    );
}
