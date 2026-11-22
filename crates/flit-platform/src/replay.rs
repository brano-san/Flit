use crate::{FileOps, Job, OpOutcome, PlatformError, CANCELLED};
use flit_core::i18n::S;
use flit_core::provider::{FileOp, OpCtl, OpReport};
use flit_core::undo::{self, Replay, Stamp, Step};
use flit_core::FsPath;
use std::collections::HashSet;
use std::io;

#[derive(Clone, Copy)]
pub struct FileSystem {
    pub stamp: fn(&FsPath) -> io::Result<(Stamp, bool)>,
    pub children: fn(&FsPath) -> io::Result<Vec<FsPath>>,
}

fn newer_descendants(fs: FileSystem, root: &FsPath, cutoff: i128, device: u64, job: &Job, changed: &mut Vec<FsPath>) -> io::Result<()> {
    let mut work = (fs.children)(root)?;
    while let Some(path) = work.pop() {
        if job.cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, CANCELLED));
        }
        let (stamp, directory) = (fs.stamp)(&path)?;
        if stamp.device != device || stamp.modified > cutoff {
            changed.push(path);
        } else if directory {
            work.extend((fs.children)(&path)?);
        }
    }
    Ok(())
}

fn make_step(steps: Vec<FileOp>, fs: FileSystem) -> Step {
    let captured_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|elapsed| elapsed.as_nanos() as i128);
    let Some(paths) = undo::check_paths(&steps) else {
        return Step::Refuse(S::ErrorsOpsUndoUnavailable.en());
    };
    let mut checks = Vec::with_capacity(paths.len());
    for (path, exists) in paths {
        let stamp = if exists {
            let Ok((stamp, _)) = (fs.stamp)(&path) else {
                return Step::Refuse(S::ErrorsOpsUndoStamp.en());
            };
            Some(stamp)
        } else {
            None
        };
        checks.push((path, stamp));
    }
    Step::Checked(Replay { steps, checks, recheck_tree: false, captured_ns })
}

fn steps(report: &OpReport) -> Option<Result<Vec<FileOp>, &'static str>> {
    let mut ops = Vec::new();
    for step in undo::inverse_ops(report)? {
        match step {
            Step::Run(FileOp::Delete(paths)) => ops.push(FileOp::Trash(paths)),
            Step::Run(op) => ops.push(op),
            Step::Refuse(message) => return Some(Err(message)),
            Step::Checked(_) => unreachable!(),
        }
    }
    Some(Ok(ops))
}

pub fn report(op: FileOp, items: Vec<(FsPath, FsPath)>, fs: Option<FileSystem>, job: &Job) -> OpReport {
    let mut report = OpReport::new(job.take_performed().unwrap_or(op), items);
    report.undo.replaced = job.take_replaced();
    report.undo.unverified = job.take_unverified();
    attach(&mut report, fs);
    report
}

fn attach(report: &mut OpReport, fs: Option<FileSystem>) {
    if let (Some(fs), Some(steps)) = (fs, steps(report)) {
        report.undo.checked = match steps {
            Ok(ops) if undo::check_paths(&ops).is_none() => None,
            Ok(ops) => Some(make_step(ops, fs)),
            Err(message) => Some(Step::Refuse(message)),
        };
    }
}

pub fn refused(paths: &[FsPath]) -> PlatformError {
    let paths = paths.iter().map(|p| p.display().into_owned()).collect::<Vec<_>>().join("\n");
    PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsUndoChanged, paths = paths))
}

pub fn run(sh: &mut impl FileOps, replay: &Replay, fs: Option<FileSystem>, job: &Job) -> (OpOutcome, Option<Step>) {
    let (mut result, next) = execute(sh, replay, fs, job);
    if replay.steps.len() < 2 || result.error.is_none() || result.items.is_empty() {
        return (result, next);
    }
    let Some(Step::Checked(rollback)) = next else {
        return (result, Some(Step::Refuse(S::ErrorsOpsUndoUnavailable.en())));
    };
    let recovery = Job::new(OpCtl::checked(rollback.clone()), Box::new(|_| {}), Box::new(|_| {}));
    let (restored, _) = execute(sh, &rollback, fs, &recovery);
    if restored.error.is_none() {
        result.items.clear();
        return (result, None);
    }
    result.items.extend(restored.items);
    (result, Some(Step::Refuse(S::ErrorsOpsUndoUnavailable.en())))
}

fn execute(sh: &mut impl FileOps, replay: &Replay, fs: Option<FileSystem>, job: &Job) -> (OpOutcome, Option<Step>) {
    let Some(fs) = fs else {
        return (OpOutcome { error: Some(PlatformError::new(-1, S::ErrorsOpsUndoUnavailable.en())), ..Default::default() }, None);
    };
    let mut changed = Vec::new();
    let restored: HashSet<_> = replay
        .steps
        .iter()
        .filter_map(|step| match step {
            FileOp::Restore(pairs) => Some(pairs.iter().map(|(source, _)| source)),
            _ => None,
        })
        .flatten()
        .collect();
    for (path, expected) in &replay.checks {
        if job.cancelled() {
            return (OpOutcome { error: Some(PlatformError::new(-1, CANCELLED)), ..Default::default() }, None);
        }
        let matches = match ((fs.stamp)(path), expected) {
            (Ok((current, true)), Some(expected)) => {
                let unchanged = if replay.recheck_tree { (current.device, current.file, current.access) == (expected.device, expected.file, expected.access) } else { current == *expected };
                unchanged && (restored.contains(path) || replay.captured_ns.is_none_or(|cutoff| newer_descendants(fs, path, cutoff, current.device, job, &mut changed).is_ok()))
            }
            (Ok((current, _)), Some(expected)) => current == *expected,
            (Err(e), None) => e.kind() == io::ErrorKind::NotFound,
            _ => false,
        };
        if !matches {
            changed.push(path.clone());
        }
    }
    for step in &replay.steps {
        if let FileOp::Restore(pairs) = step {
            for (backup, original) in pairs {
                if !sh.can_restore(backup, original) && !changed.contains(backup) {
                    changed.push(backup.clone());
                }
            }
        }
    }
    if job.cancelled() {
        return (OpOutcome { error: Some(PlatformError::new(-1, CANCELLED)), ..Default::default() }, None);
    }
    if !changed.is_empty() {
        return (OpOutcome { error: Some(refused(&changed)), ..Default::default() }, None);
    }
    let mut out = OpOutcome::default();
    let mut backwards = Vec::new();
    let mut unavailable = false;
    for op in &replay.steps {
        if job.cancelled() {
            out.error = Some(PlatformError::new(-1, CANCELLED));
            break;
        }
        let result = sh.file_op(op, job);
        let expected = match op {
            FileOp::MoveTo(pairs) | FileOp::Restore(pairs) => pairs.len(),
            FileOp::Trash(paths) | FileOp::Delete(paths) => paths.len(),
            FileOp::Access(changes) => changes.len(),
            _ => 1,
        };
        let complete = result.items.len() == expected;
        let report = report(op.clone(), result.items, None, job);
        if let Some(inverse) = steps(&report) {
            match inverse {
                Ok(ops) => backwards.push(ops),
                Err(_) => unavailable = true,
            }
        }
        out.items.extend(report.items);
        if result.error.is_some() {
            out.error = result.error;
            break;
        }
        if !complete {
            out.error = Some(PlatformError::new(-1, S::ErrorsOpsUndoFailed.en()));
            break;
        }
    }
    let next = if unavailable {
        Some(Step::Refuse(S::ErrorsOpsUndoUnavailable.en()))
    } else if backwards.is_empty() {
        None
    } else {
        Some(make_step(backwards.into_iter().rev().flatten().collect(), fs))
    };
    (out, next)
}
