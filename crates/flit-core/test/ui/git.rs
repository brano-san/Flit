use super::*;
use crate::git::Commit;
use crate::provider::{ProviderEvent, ProviderRequest};
use crate::text::Gfx;
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::{MenuItem, Persisted, ShellMenu};
use std::time::{Duration, Instant};

fn app_in(path: &str, names: &[&str]) -> (App, Vec<Effect>) {
    let mut app = App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta::default());
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    let fx = app.drain_effects().collect();
    (app, fx)
}

fn git_requests(fx: &[Effect]) -> Vec<GitRequest> {
    fx.iter().filter_map(|e| if let Effect::Git(r) = e { Some(r.clone()) } else { None }).collect()
}

fn repo(app: &App) -> GitEvent {
    let mut info = GitInfo { root: FsPath::parse(r"C:\x"), branch: "main".into(), ..GitInfo::default() };
    info.status.insert(b"a.txt".to_vec(), GitStatus::Modified);
    info.commits.insert(b"a.txt".to_vec(), Arc::new(Commit { hash: "abc".into(), author: "Ann".into(), time: crate::git::filetime(0), message: "Fix it".into() }));
    GitEvent::Status { tab: app.tab().id, gen: app.tab().gen, info: Some(Arc::new(info)) }
}

#[test]
fn local_folders_ask_for_status_and_others_do_not() {
    let (_, fx) = app_in(r"C:\x", &["a.txt"]);
    assert!(matches!(git_requests(&fx).as_slice(), [GitRequest::Status { commits: false, changed: false, .. }]));
    let (_, fx) = app_in(r"C:\x\pack.zip\inner", &[]);
    assert!(git_requests(&fx).is_empty(), "archives are never repositories");
}

#[test]
fn repository_unlocks_columns_branch_and_badges() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt", "b.txt"]);
    assert_eq!(app.git_branch(), None);
    assert!(!app.column_kinds().contains(&Column::GitStatus));
    let ev = repo(&app);
    app.on_git(ev);
    assert_eq!(app.git_branch(), Some("main"));
    assert!(app.column_kinds().contains(&Column::GitStatus));
    assert!(!app.columns().contains(&Column::GitStatus), "Git columns are opt-in");
    app.run_action("view.column.git_status", Instant::now());
    assert!(app.columns().contains(&Column::GitStatus));
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Status { commits: false, .. }]));
    app.run_action("view.column.git_message", Instant::now());
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Status { commits: true, .. }]));
    let ev = repo(&app);
    app.on_git(ev);
    let arena = bumpalo::Bump::new();
    let v = app.tab().view.as_ref().unwrap();
    let a = (0..2).find(|&i| v.listing().name(i) == b"a.txt").unwrap();
    assert_eq!(app.cell_text(&arena, v.listing(), a, Column::GitStatus), "Modified");
    assert_eq!(app.cell_text(&arena, v.listing(), a, Column::GitMessage), "Fix it");
    assert_eq!(app.cell_text(&arena, v.listing(), 1 - a, Column::GitMessage), "");
    let mut g = Gfx::new(crate::text::tests::Mono, 2048);
    app.frame(&mut g, Instant::now());
    let with_badge = g.dl.instances.len();
    app.tab_mut().git = None;
    app.frame(&mut g, Instant::now());
    assert!(g.dl.instances.len() < with_badge, "the badge is drawn over the icon");
}

#[test]
fn changes_refresh_only_repositories() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
    let (tab, gen) = (app.tab().id, app.tab().gen);
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: Vec::new() }, Instant::now());
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
    let ev = repo(&app);
    app.on_git(ev);
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: Vec::new() }, Instant::now());
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Status { changed: true, .. }]));
}

#[test]
fn branch_menu_lists_branches_and_switches() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
    let ev = repo(&app);
    app.on_git(ev);
    app.run_action("git.branches", Instant::now());
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Branches { .. }]));
    let tab = app.tab().id;
    app.on_git(GitEvent::Branches { tab, branches: vec!["main".into(), "dev".into(), "origin/main".into()] });
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(ShellMenu { menu, .. }) = e { Some(menu) } else { None }).unwrap();
    let labels: Vec<(String, bool)> = menu.iter().filter_map(|m| if let MenuItem::Command { label, checked, .. } = m { Some((label.clone(), *checked)) } else { None }).collect();
    assert_eq!(labels[..3], [("main".into(), true), ("dev".into(), false), ("origin/main".into(), false)]);
    assert!(labels.iter().any(|(l, _)| l == "Git: Pull"));
    app.menu_command(1, Instant::now());
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Run { op: GitOp::Switch(b), .. }] if b == "dev"));
}

#[test]
fn operations_need_a_repository_except_init_and_clone() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.run_action("git.pull", Instant::now());
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("This folder is not in a Git repository"));
    app.run_action("git.init", Instant::now());
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Run { op: GitOp::Init, .. }]));
    app.run_action("git.clone", Instant::now());
    assert!(app.jobs.form.is_some());
    app.form_submit(0);
    assert!(app.jobs.form.is_some(), "an empty address keeps the dialog open");
    let tab = app.tab().id;
    app.on_git(GitEvent::Done { tab, op: GitOp::Push, error: Some("fatal: no remote".into()), url: None });
    assert_eq!(app.tab().error.as_deref(), Some("Git: fatal: no remote"));
    app.on_git(GitEvent::Done { tab, op: GitOp::OpenRemote, error: None, url: Some("https://github.com/o/r".into()) });
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Launch { launch: crate::provider::Launch::Run { program, .. }, .. }, .. } if program == "https://github.com/o/r")));
}
