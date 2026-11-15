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
    app.set_git_integration(true);
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

#[test]
fn sync_is_listed_but_disabled_without_an_upstream() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    let menu = app.build_branch_menu();
    assert!(menu.iter().any(|m| matches!(m, MenuItem::Command { label, disabled: true, .. } if label.contains("Sync"))));
    assert!(!app.available("git.sync"));
    assert!(app.run_action("git.sync", Instant::now()));
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("No upstream branch"));
}

#[test]
fn sync_is_a_discoverable_palette_command() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.open_palette();
    let palette = app.nav.palette.as_ref().unwrap();
    assert!(palette.ids.iter().any(|&i| crate::ui::actions::ACTIONS[i].id == "git.sync"));
}

#[test]
fn sync_stays_disabled_in_the_palette_at_narrow_widths_in_every_language() {
    use crate::i18n::{self, Lang};
    for lang in Lang::ALL {
        i18n::set_lang(lang);
        let (mut app, _) = app_in(r"C:\x", &[]);
        app.on_event(&crate::input::Event::Resized { width: 480, height: 360 }, Instant::now());
        app.on_git(repo(&app));
        let menu = app.build_branch_menu();
        let bounds = crate::render::Rect::new(0.0, 0.0, 480.0, 360.0);
        let view = crate::ui::popup::MenuView::new(&menu, 460.0, 340.0, bounds, 1.0, &mut Gfx::new(crate::text::tests::Mono, 2048));
        assert!(view.panels().all(|r| r.x >= 0.0 && r.y >= 0.0 && r.right() <= 480.0 && r.bottom() <= 360.0));
        app.open_palette();
        for c in S::ActionsGitSync.t().chars() {
            app.on_event(&crate::input::Event::Char(c), Instant::now());
        }
        app.frame(&mut Gfx::new(crate::text::tests::Mono, 2048), Instant::now());
        let p = app.nav.palette.as_ref().unwrap();
        assert!(p.rect.x >= 0.0 && p.rect.right() <= 480.0 && p.rect.bottom() <= 360.0);
        let k = p.ids.iter().position(|&i| crate::ui::actions::ACTIONS[i].id == "git.sync").unwrap();
        assert!(!p.commands.as_ref().unwrap().live[k]);
        assert!((0..p.matches.len()).any(|row| p.row_action(row).is_some_and(|i| crate::ui::actions::ACTIONS[i].id == "git.sync")));
    }
    i18n::set_lang(Lang::En);
}

#[test]
fn sync_uses_the_active_tabs_upstream_and_does_not_leak_to_other_windows() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    let mut ev = repo(&app);
    if let GitEvent::Status { info: Some(i), .. } = &mut ev {
        Arc::make_mut(i).upstream = Some("origin/topic/x".into());
    }
    app.on_git(ev);
    assert!(app.available("git.sync"));
    assert!(app.build_branch_menu().iter().any(|m| matches!(m, MenuItem::Command { label, disabled: false, .. } if label == "Git: Sync")));
    app.run_action("git.sync", Instant::now());
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Run { op: GitOp::Sync, tab, .. }] if *tab == app.tab().id));
    let (other, _) = app_in(r"C:\x", &[]);
    assert!(!other.available("git.sync"));
    app.run_action("tab.new", Instant::now());
    assert!(!app.available("git.sync"));
    app.run_action("tab.previous", Instant::now());
    assert!(app.available("git.sync"));
}

#[test]
fn integration_is_off_by_default_and_late_status_is_ignored_after_disabling() {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    assert!(!app.prefs.git_integration);
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
    app.set_git_integration(true);
    assert!(!git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
    let status = repo(&app);
    app.set_git_integration(false);
    app.on_git(status);
    assert!(app.git_info().is_none());
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

fn terminals(app: &mut App) -> Vec<Vec<FsPath>> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch: crate::provider::Launch::Terminal { dirs, .. }, .. }, .. } = e { Some(dirs) } else { None }).collect()
}

#[test]
fn a_sign_in_failure_offers_a_terminal_in_the_repository() {
    let (mut app, _) = app_in(r"C:\x\sub", &["a.txt"]);
    let mut status = repo(&app);
    if let GitEvent::Status { info: Some(i), .. } = &mut status {
        Arc::make_mut(i).root = FsPath::parse(r"C:\x");
    }
    app.on_git(status);
    let tab = app.tab().id;
    let auth = "fatal: could not read Username for 'https://github.com': terminal prompts disabled";
    app.on_git(GitEvent::Done { tab, op: GitOp::Push, error: Some(auth.into()), url: None });
    let form = app.jobs.form.as_ref().expect("a sign-in failure asks what to do");
    assert!(matches!(&form.kind, Kind::GitAuth { dir } if *dir == FsPath::parse(r"C:\x").unwrap()));
    app.drain_effects().for_each(drop);
    app.form_submit(0);
    assert_eq!(terminals(&mut app), [vec![FsPath::parse(r"C:\x").unwrap()]], "the terminal opens at the repository root");
    assert!(app.jobs.form.is_none());
    app.on_git(GitEvent::Done { tab, op: GitOp::Pull, error: Some("git@github.com: Permission denied (publickey).".into()), url: None });
    assert!(app.jobs.form.is_some());
    app.form_submit(1);
    assert!(terminals(&mut app).is_empty(), "Cancel opens nothing");
}

#[test]
fn a_missing_git_says_how_to_install_it_and_other_failures_stay_as_they_were() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    let tab = app.tab().id;
    app.on_git(GitEvent::Done { tab, op: GitOp::Init, error: Some(S::GitErrorMissing.en().into()), url: None });
    assert_eq!(app.tab().error.as_deref(), Some(S::GitErrorInstallWindows.t()));
    app.caps.unix_paths = true;
    app.on_git(GitEvent::Done { tab, op: GitOp::Fetch, error: Some(S::GitErrorMissing.en().into()), url: None });
    assert_eq!(app.tab().error.as_deref(), Some(S::GitErrorInstallLinux.t()));
    app.on_git(GitEvent::Done { tab, op: GitOp::Switch("dev".into()), error: Some("fatal: Authentication failed".into()), url: None });
    assert!(app.jobs.form.is_none(), "a switch never talks to a server, so there is nothing to sign in to");
    assert_eq!(app.tab().error.as_deref(), Some("Git: fatal: Authentication failed"));
    app.on_git(GitEvent::Done { tab, op: GitOp::Push, error: Some("error: failed to push some refs".into()), url: None });
    assert!(app.jobs.form.is_none());
    assert_eq!(crate::git::failure("remote: Invalid username or password."), crate::git::Failure::Auth);
    assert_eq!(crate::git::failure("fatal: not a git repository"), crate::git::Failure::Other);
    assert_eq!(crate::git::op_command(&GitOp::Clone { url: "https://h/r.git".into() }).as_deref(), Some("git clone https://h/r.git"));
}
