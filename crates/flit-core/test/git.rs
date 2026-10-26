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
fn log_keeps_the_newest_commit_per_child() {
    let out = b"\x1eaaa\x1fAnn\x1f1700000000\x1fsecond\0\nb/x.txt\0top.txt\0\x1ebbb\x1fBob\x1f1600000000\x1ffirst\0\nb/y.txt\0old.txt\0";
    let mut commits = FxHashMap::default();
    let mut p = LogParser::new();
    for f in out.split(|&b| b == 0) {
        p.feed(f, &mut commits);
    }
    assert_eq!(commits[&b"b"[..]].message, "second");
    assert_eq!(commits[&b"top.txt"[..]].author, "Ann");
    assert_eq!(commits[&b"old.txt"[..]].hash, "bbb");
    assert_eq!(commits[&b"old.txt"[..]].time, filetime(1_600_000_000));
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
}
