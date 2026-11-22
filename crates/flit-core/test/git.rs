use super::*;

#[test]
fn status_maps_children_of_the_folder() {
    let out = b" M a/b/x.txt\0?? a/new.txt\0?? a/fresh/\0A  a/added.txt\0R  a/to.txt\0a/from.txt\0UU a/c.txt\0 M other.txt\0";
    let m = parse_status(out, b"a/");
    assert_eq!(m.get(&b"b"[..]), Some(&GitStatus::Modified));
    assert_eq!(m.get(&b"new.txt"[..]), Some(&GitStatus::Untracked));
    assert_eq!(m.get(&b"fresh"[..]), Some(&GitStatus::Untracked));
    assert_eq!(m.get(&b"added.txt"[..]), Some(&GitStatus::Added));
    assert_eq!(m.get(&b"to.txt"[..]), Some(&GitStatus::Renamed));
    assert_eq!(m.get(&b"from.txt"[..]), None);
    assert_eq!(m.get(&b"c.txt"[..]), Some(&GitStatus::Conflict));
    assert_eq!(m.len(), 6);
}

#[test]
fn folder_takes_the_strongest_status() {
    let m = parse_status(b"?? d/u.txt\0A  d/n.txt\0 M d/m.txt\0", b"");
    assert_eq!(m.get(&b"d"[..]), Some(&GitStatus::Modified));
    let m = parse_status(b"?? d/u.txt\0?? d/v.txt\0", b"");
    assert_eq!(m.get(&b"d"[..]), Some(&GitStatus::Untracked));
}

#[test]
fn porcelain_branch_header_does_not_turn_into_a_file_and_gone_upstream_is_unavailable() {
    let out = b"## topic/x...origin/topic/x [ahead 2, behind 5]\0 M f.txt\0";
    assert_eq!(parse_tracking(out), (Some("origin/topic/x".into()), 2, 5));
    let files = parse_status(out, b"");
    assert_eq!(files.len(), 1);
    assert_eq!(files.get(&b"f.txt"[..]), Some(&GitStatus::Modified));
    assert_eq!(parse_tracking(b"## main...origin/main [gone]\0"), (None, 0, 0));
    assert_eq!(parse_tracking(b"## No commits yet on main\0"), (None, 0, 0));
    assert_eq!(parse_shortstat(b" 1 file changed, 1 insertion(+), 2 deletions(-)\n"), (1, 2));
    assert_eq!(parse_shortstat(b" 1 file changed\n"), (0, 0));
}

#[test]
fn log_keeps_the_newest_commit_per_child() {
    let out = b"\x1eaaaaaaa1111111111111111111111111111111111\x1faaaaaaa\x1fAnn\x1f1700000000\x1fsecond\0\nb/x.txt\0top.txt\0\x1ebbbbbbb2222222222222222222222222222222222\x1fbbbbbbb\x1fBob\x1f1600000000\x1ffirst\0\nb/y.txt\0old.txt\0";
    let mut commits = FxHashMap::default();
    let mut p = LogParser::new();
    for f in out.split(|&b| b == 0) {
        p.feed(f, &mut commits);
    }
    assert_eq!(commits[&b"b"[..]].message, "second");
    assert_eq!(commits[&b"top.txt"[..]].author, "Ann");
    assert_eq!(commits[&b"old.txt"[..]].hash, "bbbbbbb2222222222222222222222222222222222");
    assert_eq!(commits[&b"old.txt"[..]].short_hash, "bbbbbbb");
    assert_eq!(commits[&b"old.txt"[..]].time, filetime(1_600_000_000));
}

#[test]
fn log_hashes_stay_seven_characters_and_incomplete_headers_do_not_reuse_a_commit() {
    let mut commits = FxHashMap::default();
    let mut parser = LogParser::new();
    parser.feed(b"\x1e0123456789abcdef0123456789abcdef01234567\x1f0123456789\x1fAnn\x1f1700000000\x1fMessage", &mut commits);
    parser.feed(b"file.txt", &mut commits);
    assert_eq!(commits[&b"file.txt"[..]].short_hash, "0123456");
    assert_eq!(commits[&b"file.txt"[..]].hash, "0123456789abcdef0123456789abcdef01234567");
    parser.feed(b"\x1eincomplete", &mut commits);
    parser.feed(b"broken.txt", &mut commits);
    assert!(!commits.contains_key(&b"broken.txt"[..]));
}

#[test]
fn head_names_the_branch_or_short_hash() {
    assert_eq!(head_branch(b"ref: refs/heads/main\n").as_deref(), Some("main"));
    assert_eq!(head_branch(b"ref: refs/heads/feature/x").as_deref(), Some("feature/x"));
    assert_eq!(head_branch(b"0123456789abcdef0123456789abcdef01234567\n").as_deref(), Some("0123456"));
    assert_eq!(head_branch(b"garbage"), None);
    assert_eq!(gitdir_link(b"gitdir: ../.git/worktrees/w\n"), Some(&b"../.git/worktrees/w"[..]));
}

#[test]
fn remotes_become_web_urls() {
    assert_eq!(web_url("git@github.com:o/r.git").as_deref(), Some("https://github.com/o/r"));
    assert_eq!(web_url("https://github.com/o/r.git").as_deref(), Some("https://github.com/o/r"));
    assert_eq!(web_url("https://user:tok@gitlab.com/g/s/r").as_deref(), Some("https://gitlab.com/g/s/r"));
    assert_eq!(web_url("ssh://git@host.org:2222/o/r.git").as_deref(), Some("https://host.org/o/r"));
    assert_eq!(web_url("C:/repos/bare.git"), None);
    assert_eq!(web_url("/srv/bare.git"), None);
    assert_eq!(repo_name("https://github.com/o/flit.git").as_deref(), Some("flit"));
    assert_eq!(repo_name("git@github.com:o/r").as_deref(), Some("r"));
    assert_eq!(repo_name("C:\\tmp\\bare.git").as_deref(), Some("bare"));
    assert_eq!(repo_name("git@host:project/.git").as_deref(), Some("project"));
    assert_eq!(repo_name("/tmp/long  name.git").as_deref(), Some("long name"));
}

#[test]
fn repository_drops_accept_only_clone_links_or_repository_pages() {
    for (url, want) in [
        ("https://github.com/a/b?tab=readme-ov-file#top", "https://github.com/a/b"),
        ("https://gitlab.com/a/b/", "https://gitlab.com/a/b"),
        ("https://codeberg.org/a/b", "https://codeberg.org/a/b"),
        ("https://bitbucket.org/a/b", "https://bitbucket.org/a/b"),
        ("https://example.com/a/group/b.git", "https://example.com/a/group/b.git"),
        ("git@host:user/repo", "git@host:user/repo"),
    ] {
        assert_eq!(dropped_repo_url(url), Some(want));
    }
    for url in [
        "https://github.com/a/b/tree/main", "https://github.com/a/b/issues?tab=all",
        "https://github.com.evil/a/b", "https://github.com@evil/a/b",
        "https://example.com/a/b", "https://github.com/a", "https://github.com/a//b",
        "git@host:user", "git@:user/repo", "file:///tmp/repo.git", "C:\\tmp\\repo.git",
        "https://github.com/a/b\nhttps://github.com/c/d",
    ] {
        assert_eq!(dropped_repo_url(url), None, "{url}");
    }
}

#[test]
fn github_login_checks_the_remote_authority() {
    for url in ["https://github.com/owner/repo", "ssh://git@GitHub.com:22/owner/repo", "git@github.com:owner/repo"] {
        assert!(github_remote(url));
    }
    for url in ["https://gitlab.com/o/r", "https://github.com.evil/o/r", "https://example.com/o/tag@github.com/r", "https://github.com@evil.test/o/r", "C:/github.com", "file://github.com/o/r"] {
        assert!(!github_remote(url), "{url}");
    }
}
