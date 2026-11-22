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
fn switching_with_changes_asks_twice_before_discard_and_can_be_cancelled() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    let tab = app.tab().id;
    let dir = app.tab().location().path().clone();
    let blocked = GitEvent::SwitchBlocked { tab, dir, from: "main".into(), branch: "dev".into() };
    for (selected, choice) in [(0, SwitchChoice::Merge), (1, SwitchChoice::Stash), (2, SwitchChoice::Discard)] {
        app.on_git(blocked.clone());
        let f = app.dialogs.form.as_mut().unwrap();
        let Control::Choice { sel, .. } = &mut f.controls[1] else { panic!("switch choices") };
        *sel = selected;
        app.form_submit(0);
        if choice == SwitchChoice::Discard {
            assert!(matches!(app.dialogs.form.as_ref().unwrap().kind, Kind::GitSwitch { confirm_discard: true, .. }));
            assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
            app.form_submit(1);
            assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
            app.on_git(blocked.clone());
            let Control::Choice { sel, .. } = &mut app.dialogs.form.as_mut().unwrap().controls[1] else { panic!("switch choices") };
            *sel = selected;
            app.form_submit(0);
            app.form_submit(0);
        }
        assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(),
            [GitRequest::Run { op: GitOp::RetrySwitch { branch, choice: got, .. }, .. }] if branch == "dev" && *got == choice));
    }
}

#[test]
fn dropping_a_repository_prefills_clone_and_other_urls_still_save_a_link() {
    use super::super::{drop_effect, DragEvent, Mods};
    let (mut app, _) = app_in(r"C:\x", &[]);
    let (x, y) = (300.0, app.layout.list.y + 200.0);
    let now = Instant::now();
    let enter = DragEvent::Enter { paths: vec![], x, y, mods: Mods::default(), allowed: drop_effect::COPY, right: false };
    let url = "https://github.com/owner/repo";
    let dropped = DragEvent::DropRepository { x, y, url: url.into() };
    app.drag(&enter, now);
    assert_eq!(app.drag(&dropped, now), drop_effect::COPY);
    let f = app.dialogs.form.as_ref().unwrap();
    assert_eq!(f.text(0), url);
    assert!(matches!(&f.kind, Kind::GitClone { dir, .. } if dir.display() == r"C:\x"));
    app.form_submit(1);
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Git(GitRequest::RunTracked { .. }) | Effect::Provider { req: ProviderRequest::Op { .. }, .. })));
    app.drag(&enter, now);
    app.drag(&dropped, now);
    app.form_submit(0);
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(),
        [GitRequest::RunTracked { op: GitOp::Clone { url: got }, dir, .. }] if got == url && dir.display() == r"C:\x"));
    app.drag(&enter, now);
    app.drag(&DragEvent::Drop { x, y, mods: Mods::default(), allowed: drop_effect::COPY }, now);
    assert!(app.dialogs.form.is_none());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: crate::provider::FileOp::PasteDropped { .. }, .. }, .. })));
}

#[test]
fn git_drop_uses_the_destination_tab_and_leaves_archive_links_to_the_platform() {
    use super::super::{drop_effect, DragEvent, Hit};
    let (mut app, _) = app_in(r"C:\x", &["pack.zip"]);
    let now = Instant::now();
    let mut gfx = Gfx::new(crate::text::tests::Mono, 2048);
    app.frame(&mut gfx, now);
    let (x, y) = (app.layout.list.x + 30.0, app.layout.list.y + 5.0);
    assert_eq!(app.hit(x, y), Hit::Row(0));
    let url = "https://github.com/owner/repo";
    assert_eq!(app.drag(&DragEvent::DropRepository { x, y, url: url.into() }, now), drop_effect::NONE);
    assert!(app.dialogs.form.is_none());
    let current = app.tabs.cur;
    app.dispatch(vec![crate::open::Route { folder: Location::parse(r"C:\other").unwrap(), select: None }], crate::open::OpenTarget::NewTab, now);
    let other = app.tab().id;
    let slot = app.tabs.cur;
    app.select_tab(current);
    app.frame(&mut gfx, now);
    let r = app.layout.tabs[slot].0;
    let (x, y) = (r.x + 10.0, r.y + r.h / 2.0);
    assert_eq!(app.hit(x, y), Hit::Tab(slot));
    assert_eq!(app.drag(&DragEvent::DropRepository { x, y, url: url.into() }, now), drop_effect::COPY);
    assert!(matches!(&app.dialogs.form.as_ref().unwrap().kind, Kind::GitClone { tab, dir } if *tab == other && dir.display() == r"C:\other"));
}

#[test]
fn git_clone_status_uses_the_url_while_operations_show_the_repository_name() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    let tab = app.tab().id;
    let url = "https://example.test/owner/project.git";
    let op = GitOp::Clone { url: url.into() };
    app.git_run(op.clone());
    assert!(app.job_states()[0].0.contains("project"));
    assert!(!app.job_states()[0].0.contains(url));
    app.on_git(GitEvent::Started { tab, op });
    assert!(app.tab().error.as_deref().is_some_and(|status| status.contains(url)));
}

#[test]
fn git_narrow_bottom_bar_keeps_files_folders_and_the_filter_minimum() {
    use super::super::bottom::BottomHit;
    let (mut app, _) = app_in(r"C:\x", &["one", "two"]);
    app.on_event(&crate::input::Event::Resized { width: 480, height: 360 }, Instant::now());
    let mut gfx = Gfx::new(crate::text::tests::Mono, 2048);
    app.frame(&mut gfx, Instant::now());
    let baseline_width = app.nav.filter.field.w;
    app.on_git(repo(&app));
    app.frame(&mut gfx, Instant::now());
    for hit in [BottomHit::Files, BottomHit::Folders] {
        assert!(app.layout.bar.iter().any(|(kind, _)| *kind == hit), "{hit:?}");
    }
    assert_eq!(app.nav.filter.field.w, baseline_width);
}

#[test]
fn an_in_progress_git_sequence_offers_abort_without_running_it_immediately() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
    let mut ev = repo(&app);
    if let GitEvent::Status { info: Some(i), .. } = &mut ev { Arc::make_mut(i).sequence = Some(crate::git::Sequence::Merge); }
    app.on_git(ev);
    assert!(app.run_action("git.abort", Instant::now()));
    assert!(app.dialogs.form.is_some());
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
}

#[test]
fn a_repository_without_a_sequence_never_offers_destructive_abort() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    assert!(app.run_action("git.abort", Instant::now()));
    assert!(app.dialogs.form.is_none());
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
}

fn sequence_event(app: &App, sequence: Sequence) -> GitEvent {
    let mut ev = repo(app);
    if let GitEvent::Status { info: Some(i), .. } = &mut ev {
        let info = Arc::make_mut(i);
        info.sequence = Some(sequence);
        info.conflicts = 1;
        info.status.insert(b"a.txt".to_vec(), GitStatus::Conflict);
    }
    ev
}

#[test]
fn abort_requires_confirmation_preserves_errors_and_removes_the_bar_after_refresh() {
    for sequence in [Sequence::Merge, Sequence::Rebase, Sequence::CherryPick] {
        let (mut app, _) = app_in(r"C:\x", &["a.txt", "b.txt"]);
        app.on_git(sequence_event(&app, sequence));
        assert!(app.layout.git_bar.iter().all(|r| r.w > 0.0));
        app.ask_git_abort();
        let f = app.dialogs.form.as_ref().unwrap();
        assert_eq!(f.focus, f.controls.len() + 1);
        app.form_submit(1);
        assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
        app.ask_git_abort();
        app.form_submit(0);
        let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
        assert!(matches!(requests.as_slice(), [GitRequest::Run { dir, op: GitOp::Abort(s), .. }] if dir.display() == r"C:\x" && *s == sequence));
        app.form_submit(0);
        assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
        app.on_git(GitEvent::Done { tab: app.tab().id, op: GitOp::Abort(sequence), error: Some("permission denied".into()), url: None });
        assert!(app.dialogs.form.as_ref().unwrap().error);
        assert!(app.layout.git_bar[2].w > 0.0);
        app.form_submit(0);
        app.on_git(GitEvent::Done { tab: app.tab().id, op: GitOp::Abort(sequence), error: None, url: None });
        assert!(app.dialogs.form.is_none());
        assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).iter().any(|r| matches!(r, GitRequest::Status { .. })));
        app.on_git(repo(&app));
        assert!(app.layout.git_bar.iter().all(|r| r.w == 0.0));
        assert!(!app.available("git.abort"));
    }
}

#[test]
fn show_conflicts_selects_only_conflicts_and_keeps_other_tabs_panes_and_windows_independent() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt", "b.txt"]);
    app.on_git(sequence_event(&app, Sequence::Merge));
    app.tab_mut().view.as_mut().unwrap().set_selected(1, true);
    app.show_git_conflicts();
    assert_eq!(app.tab().view.as_ref().unwrap().selected().collect::<Vec<_>>(), [0]);
    let first = app.tab().id;
    app.run_action("tab.new", Instant::now());
    assert!(app.git_sequence().is_none());
    app.run_action("tab.previous", Instant::now());
    assert_eq!(app.tab().id, first);
    assert_eq!(app.git_sequence(), Some(Sequence::Merge));
    app.run_action("panel.split_right", Instant::now());
    if app.tab().id == first { app.run_action("panel.next", Instant::now()); }
    app.on_git(repo(&app));
    let other_pane = app.tab().id;
    assert!(app.git_sequence().is_none());
    app.run_action("panel.previous", Instant::now());
    assert_eq!(app.git_sequence(), Some(Sequence::Merge));
    app.drain_effects().for_each(drop);
    app.on_git(GitEvent::Done { tab: first, op: GitOp::Abort(Sequence::Merge), error: None, url: None });
    let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
    for id in [first, other_pane] {
        assert!(requests.iter().any(|r| matches!(r, GitRequest::Status { tab, .. } if *tab == id)));
    }
    let (mut other, _) = app_in(r"C:\x", &["a.txt"]);
    other.on_git(repo(&other));
    assert!(other.git_sequence().is_none());
    app.set_git_integration(false);
    assert!(app.git_sequence().is_none());
    assert!(app.layout.git_bar.iter().all(|r| r.w == 0.0));
}

#[test]
fn sequence_bars_and_confirmations_fit_narrow_windows_in_every_language() {
    use crate::i18n::{self, Lang};
    for lang in Lang::ALL {
        i18n::set_lang(lang);
        for sequence in [Sequence::Merge, Sequence::Rebase, Sequence::CherryPick] {
            let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
            app.on_event(&crate::input::Event::Resized { width: 480, height: 360 }, Instant::now());
            app.on_git(sequence_event(&app, sequence));
            let mut gfx = Gfx::new(crate::text::tests::Mono, 2048);
            app.frame(&mut gfx, Instant::now());
            for (label, r) in app.git_bar_labels().unwrap().into_iter().zip(app.layout.git_bar) {
                assert!(r.x >= 0.0 && r.right() <= 480.0 && r.bottom() <= app.layout.header.y, "{lang:?}");
                assert!(gfx.measure(&label, app.content_font(13.0)) <= r.w, "{lang:?}: {label}");
            }
            let r = app.layout.git_bar[2];
            assert_eq!(app.hit(r.x + 1.0, r.y + 1.0), super::super::Hit::GitAbort);
            let tree = app.accessibility();
            assert!(tree.nodes.iter().any(|n| n.name == sequence_labels(sequence)[1].t()));
            app.ask_git_abort();
            app.frame(&mut gfx, Instant::now());
            let f = app.dialogs.form.as_ref().unwrap();
            let Control::Paragraph { text, lines } = &f.controls[0] else { panic!("wrapped warning") };
            assert_eq!(lines.join("").split_whitespace().collect::<String>(), text.split_whitespace().collect::<String>());
            assert!(f.rect.x >= 0.0 && f.rect.right() <= 480.0 && f.rect.bottom() <= 360.0);
        }
    }
    i18n::set_lang(Lang::En);
}

#[test]
fn creating_a_branch_fetches_choices_in_the_background() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    assert!(app.build_branch_menu().iter().any(|m| matches!(m, MenuItem::Command { label, .. } if label == "Git: New branch…")));
    assert!(app.run_action("git.new_branch", Instant::now()));
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Branches { .. }]));
}

#[test]
fn creating_a_branch_never_opens_a_form_outside_a_repository() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    assert!(app.run_action("git.new_branch", Instant::now()));
    assert!(app.dialogs.form.is_none());
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
}

fn branch_choices(app: &App, create: bool) -> GitEvent {
    let branches = [("dev", false), ("origin/main", true), ("main", false), ("origin/topic", false)].into_iter().map(|(name, remote)| crate::git::Branch { name: name.into(), remote }).collect();
    GitEvent::Branches { tab: app.tab().id, gen: app.tab().gen, dir: app.tab().location().path().clone(), create, branches }
}

#[test]
fn new_branch_form_keeps_errors_and_uses_its_original_repository() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    app.on_git(branch_choices(&app, true));
    let tab = app.tab().id;
    let f = app.dialogs.form.as_mut().unwrap();
    assert!(f.checked(2));
    assert!(matches!(&f.kind, Kind::GitNewBranch { bases, .. } if bases == &["main", "dev", "origin/topic"]));
    f.set_text(0, "bad name");
    app.drain_effects().for_each(drop);
    app.form_submit(0);
    let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
    let [GitRequest::Run { op, dir, .. }] = requests.as_slice() else { panic!("creation request") };
    assert_eq!(dir, &FsPath::parse(r"C:\x").unwrap());
    assert_eq!(op, &GitOp::CreateBranch { name: "bad name".into(), base: "main".into(), switch: true });
    app.form_submit(0);
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty(), "a pending submit cannot start twice");
    app.on_git(GitEvent::Done { tab, op: op.clone(), error: Some("fatal: invalid branch name".into()), url: None });
    let f = app.dialogs.form.as_mut().unwrap();
    assert!(f.error && f.note.contains("invalid branch"));
    assert_eq!(f.text(0), "bad name");
    f.set_text(0, "topic/new");
    if let Control::Check { on, .. } = &mut f.controls[2] { *on = false; }
    app.drain_effects().for_each(drop);
    app.form_submit(0);
    let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
    let [GitRequest::Run { op, .. }] = requests.as_slice() else { panic!("retry request") };
    assert_eq!(op, &GitOp::CreateBranch { name: "topic/new".into(), base: "main".into(), switch: false });
    app.on_git(GitEvent::Done { tab, op: op.clone(), error: None, url: None });
    assert!(app.dialogs.form.is_none());
}

#[test]
fn deleting_an_unmerged_branch_requires_a_second_confirmation_and_defaults_to_cancel() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    app.on_git(branch_choices(&app, false));
    for k in [1, 2] {
        app.ask_git_delete(k);
        assert!(app.dialogs.form.is_none(), "remote and current branches cannot be deleted");
    }
    app.ask_git_delete(0);
    let f = app.dialogs.form.as_ref().unwrap();
    assert_eq!(f.focus, f.controls.len() + 1);
    app.drain_effects().for_each(drop);
    app.form_submit(1);
    assert!(git_requests(&app.drain_effects().collect::<Vec<_>>()).is_empty());
    app.ask_git_delete(0);
    app.form_submit(0);
    let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
    assert!(matches!(requests.as_slice(), [GitRequest::Run { op: GitOp::DeleteBranch { name, force: false }, .. }] if name == "dev"));
    let tab = app.tab().id;
    app.on_git(GitEvent::Done { tab, op: GitOp::DeleteBranch { name: "dev".into(), force: false }, error: Some("error: the branch 'dev' is not fully merged".into()), url: None });
    let f = app.dialogs.form.as_ref().unwrap();
    assert!(matches!(&f.kind, Kind::GitDeleteBranch { force: true, pending: false, .. }));
    assert_eq!(f.focus, f.controls.len() + 1);
    assert!(matches!(&f.controls[0], Control::Paragraph { text, .. } if text.contains("dev") && text.contains("may be lost")));
    assert!(!git_requests(&app.drain_effects().collect::<Vec<_>>()).iter().any(|r| matches!(r, GitRequest::Run { .. })), "force is never automatic");
    app.form_submit(0);
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::Run { op: GitOp::DeleteBranch { force: true, .. }, .. }]));
    app.on_git(GitEvent::Done { tab, op: GitOp::DeleteBranch { name: "dev".into(), force: true }, error: None, url: None });
    assert!(app.dialogs.form.is_none());
}

#[test]
fn stale_branch_choices_do_not_open_forms_in_other_tabs_or_directories() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    let old = branch_choices(&app, true);
    app.run_action("tab.new", Instant::now());
    app.on_git(old.clone());
    assert!(app.dialogs.form.is_none());
    app.run_action("tab.previous", Instant::now());
    app.tab_mut().gen += 1;
    app.on_git(old);
    assert!(app.dialogs.form.is_none());
}

#[test]
fn changing_the_interface_language_never_translates_branch_names() {
    use crate::i18n::{self, Lang};
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    let mut choices = branch_choices(&app, true);
    if let GitEvent::Branches { branches, .. } = &mut choices { branches.push(crate::git::Branch { name: "Cancel".into(), remote: false }); }
    app.on_git(choices);
    i18n::set_lang(Lang::Fr);
    app.relabel_form(Lang::En);
    let f = app.dialogs.form.as_ref().unwrap();
    assert!(matches!(&f.controls[1], Control::Choice { options, .. } if options.iter().any(|s| s == "Cancel")));
    i18n::set_lang(Lang::En);
}

#[test]
fn branch_forms_fit_a_narrow_window_with_every_translation() {
    use crate::i18n::{self, Lang};
    for lang in Lang::ALL {
        i18n::set_lang(lang);
        let (mut app, _) = app_in(r"C:\x", &[]);
        app.on_git(repo(&app));
        app.on_event(&crate::input::Event::Resized { width: 480, height: 360 }, Instant::now());
        app.on_git(branch_choices(&app, true));
        let mut gfx = Gfx::new(crate::text::tests::Mono, 2048);
        app.frame(&mut gfx, Instant::now());
        let f = app.dialogs.form.as_ref().unwrap();
        assert!(f.rect.x >= 0.0 && f.rect.right() <= 480.0 && f.rect.bottom() <= 360.0, "{lang:?}");
        assert!(f.button_rects.iter().all(|r| r.x >= f.rect.x && r.right() <= f.rect.right()), "{lang:?}");
        app.form_submit(1);
        app.ask_git_delete(0);
        app.frame(&mut gfx, Instant::now());
        let f = app.dialogs.form.as_ref().unwrap();
        assert!(f.rect.x >= 0.0 && f.rect.right() <= 480.0 && f.rect.bottom() <= 360.0, "{lang:?}");
        app.form_submit(0);
        app.on_git(GitEvent::Done { tab: app.tab().id, op: GitOp::DeleteBranch { name: "dev".into(), force: false }, error: Some("not fully merged".into()), url: None });
        app.frame(&mut gfx, Instant::now());
        let f = app.dialogs.form.as_ref().unwrap();
        let Control::Paragraph { text, lines } = &f.controls[0] else { panic!("wrapped warning") };
        assert!(!lines.is_empty(), "{lang:?}");
        assert_eq!(lines.join("").split_whitespace().collect::<String>(), text.split_whitespace().collect::<String>(), "{lang:?}");
        let font = crate::text::FontStyle::new(crate::text::FontFamily::Ui, 12.0, false);
        if gfx.measure(text, font) > f.rows[0].w - 8.0 { assert!(lines.len() > 1, "{lang:?}"); }
        assert!(lines.iter().all(|line| gfx.measure(line, font) <= f.rows[0].w - 8.0), "{lang:?}");
        assert_eq!(f.focus, f.controls.len() + 1);
    }
    i18n::set_lang(Lang::En);
}

#[test]
fn git_branch_summary_fits_alongside_the_filter_at_narrow_widths() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    let mut status = repo(&app);
    if let GitEvent::Status { info: Some(i), .. } = &mut status {
        let g = Arc::make_mut(i);
        g.branch = "feature/a-long-name-for-the-current-branch".into();
        (g.ahead, g.behind, g.insertions, g.deletions) = (3, 12, 40, 24);
        g.stashes = vec![crate::git::Stash { oid: "first".into(), message: "On main: Flit: main".into() }].into();
    }
    app.on_git(status);
    app.on_event(&crate::input::Event::Resized { width: 680, height: 360 }, Instant::now());
    let mut gfx = Gfx::new(crate::text::tests::Mono, 2048);
    app.frame(&mut gfx, Instant::now());
    let branch = app.layout.branch;
    assert!(branch.w > 0.0 && branch.right() <= 680.0, "branch {branch:?}, status {:?}", app.layout.status);
    assert!(branch.x >= app.nav.filter.field.right());
    let label = app.git_branch_label().unwrap();
    assert!(label.contains("↑3 ↓12  +40 −24"));
    assert!(label.contains(&crate::trn!(crate::i18n::P::GitStashCount, 1)));
}

#[test]
fn commit_hash_is_opt_in_and_only_available_in_a_repository() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
    assert!(!app.column_kinds().iter().any(|c| c.title() == "Commit hash"));
    app.on_git(repo(&app));
    assert!(app.column_kinds().iter().any(|c| c.title() == "Commit hash"));
    assert!(!app.columns().iter().any(|c| c.title() == "Commit hash"));
    assert!(app.run_action("view.column.git_hash", Instant::now()));
    assert!(app.columns().iter().any(|c| c.title() == "Commit hash"));
}

#[test]
fn full_commit_hash_copy_is_offered_only_for_a_committed_row_with_the_column_visible() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt", "untracked.txt"]);
    app.on_git(repo(&app));
    let v = app.tab_mut().view.as_mut().unwrap();
    v.set_selected(0, true);
    v.focus = Some(0);
    assert!(!app.build_menu(true).iter().any(|m| matches!(m, MenuItem::Command { label, .. } if label == "Copy full commit hash")));
    assert!(app.run_action("view.column.git_hash", Instant::now()));
    assert!(app.build_menu(true).iter().any(|m| matches!(m, MenuItem::Command { label, .. } if label == "Copy full commit hash")));
    let command = app.menu.iter().position(|c| matches!(c, crate::ui::menu::Cmd::CopyGitHash(_))).unwrap();
    app.drain_effects().for_each(drop);
    app.menu_command(command, Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboard(s) if s == "abcdef0123456789abcdef0123456789abcdef01")));
    let v = app.tab_mut().view.as_mut().unwrap();
    v.clear_selection();
    v.set_selected(1, true);
    v.focus = Some(1);
    assert!(!app.build_menu(true).iter().any(|m| matches!(m, MenuItem::Command { label, .. } if label == "Copy full commit hash")), "an untracked row has no commit hash");
    app.run_action("view.column.git_hash", Instant::now());
    assert!(!app.build_menu(true).iter().any(|m| matches!(m, MenuItem::Command { label, .. } if label == "Copy full commit hash")));
}

fn deliver_sort(app: &mut App) {
    let effects: Vec<_> = app.drain_effects().collect();
    for effect in effects {
        if let Effect::Resort { tab, gen, snap, view } = effect {
            let mut order = Vec::new();
            sort::sort(&snap.listing, &snap.keys, view, &mut order);
            app.on_resorted(tab, gen, view, snap.listing.len(), order);
        }
    }
}

#[test]
fn commit_hash_visibility_and_copy_follow_the_active_tab_and_pane() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
    app.on_git(repo(&app));
    app.run_action("view.column.git_hash", Instant::now());
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    let first = app.tab().id;
    assert!(app.selected_commit_hash().is_some());
    app.run_action("tab.new", Instant::now());
    assert!(app.selected_commit_hash().is_none());
    app.run_action("tab.previous", Instant::now());
    assert_eq!(app.tab().id, first);
    assert!(app.selected_commit_hash().is_some());
    app.run_action("panel.split_right", Instant::now());
    if app.tab().id == first { app.run_action("panel.next", Instant::now()); }
    assert_ne!(app.tab().id, first);
    app.on_git(repo(&app));
    if app.tab().columns.shown(Column::GitHash) { app.run_action("view.column.git_hash", Instant::now()); }
    assert!(app.selected_commit_hash().is_none());
    app.run_action("panel.previous", Instant::now());
    assert_eq!(app.tab().id, first);
    assert!(app.tab().columns.shown(Column::GitHash));
    assert!(app.selected_commit_hash().is_some());
}

#[test]
fn commit_hash_sorts_as_text_in_both_directions_and_refreshes_without_losing_selection() {
    let (mut app, _) = app_in(r"C:\x", &["a.txt", "b.txt", "untracked.txt"]);
    let mut ev = repo(&app);
    if let GitEvent::Status { info: Some(i), .. } = &mut ev {
        let g = Arc::make_mut(i);
        let first = g.commits[&b"a.txt"[..]].as_ref().clone();
        g.commits.insert(b"a.txt".to_vec(), Arc::new(Commit { short_hash: "fffffff".into(), ..first.clone() }));
        g.commits.insert(b"b.txt".to_vec(), Arc::new(Commit { short_hash: "1111111".into(), ..first }));
    }
    app.on_git(ev.clone());
    app.run_action("view.column.git_hash", Instant::now());
    assert_eq!(Column::GitHash.field(), Some(crate::sort::SortField::GitHash));
    let v = app.tab_mut().view.as_mut().unwrap();
    v.set_selected(0, true);
    v.focus = Some(0);
    app.sort_column(crate::sort::SortField::GitHash);
    deliver_sort(&mut app);
    assert_eq!(app.tab().view.as_ref().unwrap().rows(), &[2, 1, 0]);
    app.sort_column(crate::sort::SortField::GitHash);
    deliver_sort(&mut app);
    assert_eq!(app.tab().view.as_ref().unwrap().rows(), &[0, 1, 2]);
    if let GitEvent::Status { info: Some(i), .. } = &mut ev {
        Arc::make_mut(Arc::make_mut(i).commits.get_mut(&b"a.txt"[..]).unwrap()).short_hash = "0000000".into();
    }
    app.on_git(ev);
    deliver_sort(&mut app);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.rows(), &[1, 0, 2]);
    assert_eq!(v.selected().collect::<Vec<_>>(), [0]);
    assert_eq!(v.focus, Some(0));
    let arena = bumpalo::Bump::new();
    assert_eq!(app.cell_text(&arena, v.listing(), 0, Column::GitHash), "0000000");
    assert_eq!(app.cell_text(&arena, v.listing(), 2, Column::GitHash), "");
    app.set_git_integration(false);
    deliver_sort(&mut app);
    assert_eq!(app.tab().view.as_ref().unwrap().rows(), &[2, 1, 0], "without Git data the current descending text sort falls back to names");
    assert!(!app.columns().contains(&Column::GitHash));
}

#[test]
fn commit_hash_menus_fit_narrow_windows_in_every_language_and_do_not_change_other_windows() {
    use crate::i18n::{self, Lang};
    for lang in Lang::ALL {
        i18n::set_lang(lang);
        let (mut app, _) = app_in(r"C:\x", &["a.txt"]);
        app.on_git(repo(&app));
        app.run_action("view.column.git_hash", Instant::now());
        app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
        let menu = app.build_menu(true);
        assert!(menu.iter().any(|m| matches!(m, MenuItem::Command { label, .. } if label == S::GitCopyHash.t())));
        let mut gfx = Gfx::new(crate::text::tests::Mono, 2048);
        let bounds = crate::render::Rect::new(0.0, 0.0, 480.0, 360.0);
        let hash_menu: Vec<_> = menu.iter().filter(|m| matches!(m, MenuItem::Command { label, .. } if label == S::GitCopyHash.t())).cloned().collect();
        let view = crate::ui::popup::MenuView::new(&hash_menu, 460.0, 340.0, bounds, 1.0, &mut gfx);
        assert!(view.panels().all(|r| r.x >= 0.0 && r.y >= 0.0 && r.right() <= 480.0 && r.bottom() <= 360.0), "{lang:?}");
        app.on_event(&crate::input::Event::Resized { width: 480, height: 360 }, Instant::now());
        app.frame(&mut gfx, Instant::now());
        let (other, _) = app_in(r"C:\x", &["a.txt"]);
        assert!(!other.tab().columns.shown(Column::GitHash));
    }
    i18n::set_lang(Lang::En);
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
    assert!(matches!(git_requests(&app.drain_effects().collect::<Vec<_>>()).as_slice(), [GitRequest::RunTracked { op: GitOp::Sync, tab, .. }] if *tab == app.tab().id));
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
    info.commits.insert(b"a.txt".to_vec(), Arc::new(Commit { hash: "abcdef0123456789abcdef0123456789abcdef01".into(), short_hash: "abcdef0".into(), author: "Ann".into(), time: crate::git::filetime(0), message: "Fix it".into() }));
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
    let branches = vec![crate::git::Branch { name: "main".into(), remote: false }, crate::git::Branch { name: "dev".into(), remote: false }, crate::git::Branch { name: "origin/main".into(), remote: true }];
    app.on_git(GitEvent::Branches { tab, gen: app.tab().gen, dir: app.tab().location().path().clone(), create: false, branches });
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(ShellMenu { menu, .. }) = e { Some(menu) } else { None }).unwrap();
    let labels: Vec<(String, bool)> = menu.iter().filter_map(|m| if let MenuItem::Command { label, checked, .. } = m { Some((label.clone(), *checked)) } else { None }).collect();
    assert_eq!(labels[..2], [("main".into(), true), ("origin/main".into(), false)]);
    assert!(menu.iter().any(|m| matches!(m, MenuItem::Submenu { label, items } if label == "dev" && items.len() == 2)));
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
    assert!(app.dialogs.form.is_some());
    app.form_submit(0);
    assert!(app.dialogs.form.is_some(), "an empty address keeps the dialog open");
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
    let form = app.dialogs.form.as_ref().expect("a sign-in failure asks what to do");
    assert!(matches!(&form.kind, Kind::GitAuth { dir, .. } if *dir == FsPath::parse(r"C:\x").unwrap()));
    app.drain_effects().for_each(drop);
    app.form_submit(0);
    assert_eq!(terminals(&mut app), [vec![FsPath::parse(r"C:\x").unwrap()]], "the terminal opens at the repository root");
    assert!(app.dialogs.form.is_none());
    app.on_git(GitEvent::Done { tab, op: GitOp::Pull, error: Some("git@github.com: Permission denied (publickey).".into()), url: None });
    assert!(app.dialogs.form.is_some());
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
    assert!(app.dialogs.form.is_none(), "a switch never talks to a server, so there is nothing to sign in to");
    assert_eq!(app.tab().error.as_deref(), Some("Git: fatal: Authentication failed"));
    app.on_git(GitEvent::Done { tab, op: GitOp::Push, error: Some("error: failed to push some refs".into()), url: None });
    assert!(app.dialogs.form.is_none());
    assert_eq!(crate::git::failure("remote: Invalid username or password."), crate::git::Failure::Auth);
    assert_eq!(crate::git::failure("fatal: not a git repository"), crate::git::Failure::Other);
    assert_eq!(crate::git::op_command(&GitOp::Clone { url: "https://h/r.git".into() }).as_deref(), Some("git clone https://h/r.git"));
}

#[test]
fn github_sign_in_launches_the_selected_command_and_missing_tools_only_show_the_hint() {
    use crate::git::GithubLogin;
    let (mut app, _) = app_in(r"C:\x", &[]);
    let tab = app.tab().id;
    let dir = app.tab().location().path().clone();
    let event = |github| GitEvent::AuthFailed { tab, dir: dir.clone(), op: GitOp::Fetch, error: "fatal: Authentication failed".into(), github };
    let argv = ["gh", "auth", "login", "--web"].map(str::to_owned).to_vec();
    app.on_git(event(Some(GithubLogin::Command(argv.clone()))));
    assert!(app.dialogs.form.as_ref().unwrap().buttons.contains(&S::GitErrorGithubLogin.t()));
    app.form_submit(1);
    assert!(app.drain_effects().any(|e| matches!(e,
        Effect::Provider { req: ProviderRequest::Launch { launch: Launch::RunInTerminalCommand { dir: folder, argv: args }, .. }, .. }
        if folder == dir && args == argv)));
    for github in [Some(GithubLogin::Install("github-cli")), None] {
        let show_hint = github.is_some();
        app.on_git(event(github));
        let f = app.dialogs.form.as_ref().unwrap();
        assert_eq!(f.buttons, [S::GitErrorAuthOpen.t(), S::WidgetsCancel.t()]);
        assert_eq!(f.controls.iter().any(|c| matches!(c, Control::Paragraph { text, .. } if text.contains("github-cli"))), show_hint);
        app.form_submit(1);
    }
}

#[test]
fn network_git_records_progress_errors_and_cancellation_in_operations() {
    use crate::git::{GitPhase, GitProgress};
    use crate::ui::JobState;
    for op in [GitOp::Fetch, GitOp::Pull, GitOp::Push, GitOp::Sync, GitOp::Clone { url: "https://example.test/project.git".into() }] {
        let (mut app, _) = app_in(r"C:\x", &[]);
        let mut ev = repo(&app);
        if let GitEvent::Status { info: Some(i), .. } = &mut ev { Arc::make_mut(i).upstream = Some("origin/main".into()); }
        app.on_git(ev);
        app.git_run(op.clone());
        let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
        let [GitRequest::RunTracked { tab, job, .. }] = requests.as_slice() else { panic!("tracked git operation") };
        assert_eq!(app.job_states()[0].1, JobState::Running);
        app.on_git(GitEvent::Progress { tab: *tab, id: job.id, progress: GitProgress { phase: GitPhase::Receiving, percent: 61 } });
        assert_eq!(app.job_states()[0].2.items, 61);
        assert_eq!(app.job_states()[0].2.total_items, 100);
        app.pause_job(0);
        assert!(!job.ctl.is_paused());
        if op == GitOp::Pull {
            app.cancel_job(0);
            assert!(job.ctl.is_cancelled());
            app.on_git(GitEvent::TrackedDone { tab: *tab, id: job.id, op, result: GitResult::Cancelled });
            assert_eq!(app.job_states()[0].1, JobState::Cancelled);
        } else {
            let error = "fatal: upstream unavailable".to_owned();
            let result = GitResult::Failed { error: error.clone(), auth: None };
            app.on_git(GitEvent::TrackedDone { tab: *tab, id: job.id, op, result });
            assert_eq!(app.job_states()[0].1, JobState::Failed(error));
            assert_eq!(app.tab().error.as_deref(), Some("Git: fatal: upstream unavailable"));
        }
    }
}

#[test]
fn git_leftover_index_lock_requires_an_operations_action_and_confirmation() {
    use crate::git::GitIndexLock;
    use super::super::Hit;
    let (mut app, _) = app_in(r"C:\x", &[]);
    app.on_git(repo(&app));
    app.git_run(GitOp::Pull);
    let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
    let [GitRequest::RunTracked { tab, job, .. }] = requests.as_slice() else { panic!("tracked pull") };
    let lock = GitIndexLock { path: r"C:\x\.git\index.lock".into(), file_id: (1, 2), modified: std::time::SystemTime::UNIX_EPOCH, len: 0 };
    app.on_git(GitEvent::TrackedDone { tab: *tab, id: job.id, op: GitOp::Pull, result: GitResult::CancelledWithLock(lock.clone()) });
    assert_eq!(app.job_states()[0].1, crate::ui::JobState::Cancelled);
    assert!(app.dialogs.form.is_none());
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Git(GitRequest::RemoveIndexLock { .. }))));
    let now = Instant::now();
    app.jobs_click(Hit::OpsButton, now);
    for button in [1, 0] {
        app.jobs_click(Hit::OpsMore(0), now);
        app.jobs_click(Hit::OpsMenu(1), now);
        assert!(matches!(&app.dialogs.form.as_ref().unwrap().kind, Kind::GitRemoveIndexLock { lock: offered, .. } if *offered == lock));
        assert!(app.drain_effects().all(|e| !matches!(e, Effect::Git(GitRequest::RemoveIndexLock { .. }))));
        app.form_submit(button);
        let removed = app.drain_effects().any(|e| matches!(e, Effect::Git(GitRequest::RemoveIndexLock { id, lock: requested, .. }) if id == job.id && requested == lock));
        assert_eq!(removed, button == 0);
    }
    app.on_git(GitEvent::IndexLockRemoved { tab: *tab, id: job.id, error: None });
    app.jobs_click(Hit::OpsMore(0), now);
    app.jobs_click(Hit::OpsMenu(1), now);
    assert!(app.dialogs.form.is_none());
}

#[test]
fn a_failed_git_transfer_hides_the_password_in_its_address() {
    let (mut app, _) = app_in(r"C:\x", &[]);
    let ev = repo(&app);
    app.on_git(ev);
    app.git_run(GitOp::Fetch);
    let requests = git_requests(&app.drain_effects().collect::<Vec<_>>());
    let [GitRequest::RunTracked { tab, job, .. }] = requests.as_slice() else { panic!("tracked fetch") };
    let error = "fatal: unable to access 'https://ann:secret@example.test/r.git/'".to_owned();
    app.on_git(GitEvent::TrackedDone { tab: *tab, id: job.id, op: GitOp::Fetch, result: GitResult::Failed { error, auth: None } });
    let crate::ui::JobState::Failed(shown) = app.job_states()[0].1.clone() else { panic!("a failed job") };
    assert!(shown.contains("ann:***@") && !shown.contains("secret"), "{shown}");
    assert!(!app.tab().error.clone().unwrap_or_default().contains("secret"));
}
