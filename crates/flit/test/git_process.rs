use super::*;
use flit_core::git::GitPhase;

#[test]
fn chunked_git_progress_survives_carriage_returns_and_unterminated_final_lines() {
    let mut stderr = Stderr::default();
    let mut progress = Vec::new();
    let mut track = |p| progress.push(p);
    stderr.feed(b"remote: Counting objects:  35% (3/9)\rReceiving obj", &mut track);
    stderr.feed(b"ects:  45% (9/20)\r\nmalformed progress: 80%\rWriting objects: 101%\n", &mut track);
    stderr.feed(b"Resolving deltas: 100% (20/20)", &mut track);
    stderr.flush(&mut track);
    assert_eq!(progress, [
        GitProgress { phase: GitPhase::Counting, percent: 35 },
        GitProgress { phase: GitPhase::Receiving, percent: 45 },
        GitProgress { phase: GitPhase::Resolving, percent: 100 },
    ]);
}

#[test]
fn stderr_keeps_the_actual_git_error_after_progress() {
    let mut stderr = Stderr::default();
    stderr.feed(b"Writing objects: 100% (9/9)\rfatal: remote hung", &mut |_| {});
    stderr.feed(b" up unexpectedly\nEverything up-to-date\n", &mut |_| {});
    assert_eq!(stderr.error(), "fatal: remote hung up unexpectedly");
}

#[test]
fn git_small_pack_reports_unpacking_progress() {
    let mut stderr = Stderr::default();
    let mut progress = Vec::new();
    stderr.feed(b"Unpacking objects:  45% (9/20)\rUnpacking objects: 101% (21/20)\r", &mut |p| progress.push(p));
    assert_eq!(progress.iter().map(|p| p.percent).collect::<Vec<_>>(), [45]);
}
