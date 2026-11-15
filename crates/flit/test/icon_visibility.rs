use super::*;
use flit_core::ui::IconKind;

#[test]
fn queued_content_requests_follow_exact_visible_keys_even_without_a_generation() {
    let visible = Visible::default();
    let req = IconRequest { key: 7, kind: IconKind::Content, path: flit_core::FsPath::parse("/tmp/image").unwrap(), size: 16, gen: 0, row: 0 };
    assert!(!visible.wants(&req));
    *visible.content.lock().unwrap() = vec![7];
    assert!(visible.wants(&req));
    assert!(!Visible::default().wants(&req), "another window has its own visibility");
    *visible.content.lock().unwrap() = vec![9];
    assert!(!visible.wants(&req), "scrolling away cancels a queued gen=0 search or column request");
    assert!(visible.wants(&IconRequest { kind: IconKind::Type, ..req }));
}

#[derive(Default)]
struct Folders {
    prepared: Vec<Vec<u64>>,
    drawn: Vec<u64>,
}

impl Shell for Folders {
    const BATCHES_FOLDER_ICONS: bool = true;
    fn new() -> Result<Self, PlatformError> {
        Ok(Folders::default())
    }
    fn icon(&mut self, req: &IconRequest) -> Option<flit_core::ui::Bitmap> {
        self.drawn.push(req.key);
        req.key.is_multiple_of(2).then(|| flit_core::ui::Bitmap { width: 1, height: 1, rgba: vec![0; 4] })
    }
    fn prepare_icons(&mut self, reqs: &[IconRequest]) {
        self.prepared.push(reqs.iter().map(|r| r.key).collect());
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Ok(())
    }
}

struct Plain;

impl Shell for Plain {
    fn new() -> Result<Self, PlatformError> {
        Ok(Plain)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<flit_core::ui::Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Ok(())
    }
}

fn folder(key: u64, row: u32) -> IconRequest {
    IconRequest { key, kind: IconKind::CustomFolder, path: flit_core::FsPath::parse(&format!("/home/ann/d{key}")).unwrap(), size: 32, gen: 5, row }
}

#[test]
fn visible_folder_icons_are_prepared_together_and_scrolled_away_ones_are_skipped() {
    let visible = Visible::default();
    visible.pane.store((5, 0, 9));
    let mut shell = Folders::default();
    let reqs = vec![folder(2, 1), folder(3, 2), folder(4, 50), folder(6, 9)];
    let done = batch_icons(&mut shell, 7, reqs, &visible);
    assert_eq!(shell.prepared, vec![vec![2, 3, 6]], "one preparation for the visible folders only");
    assert_eq!(shell.drawn, vec![2, 3, 6]);
    let results: Vec<(u64, &str)> = done.iter().map(|d| match d {
        Done::Icon { window: 7, req, result } => (req.key, match result { IconResult::Ready(_) => "ready", IconResult::Failed => "failed", IconResult::Skipped => "skipped" }),
        _ => panic!("only icon results"),
    }).collect();
    assert_eq!(results.len(), 4, "every request gets an answer so none stays pending");
    for want in [(2, "ready"), (3, "failed"), (4, "skipped"), (6, "ready")] {
        assert!(results.contains(&want), "{want:?} in {results:?}");
    }
    let mut idle = Folders::default();
    let done = batch_icons(&mut idle, 7, vec![folder(8, 70)], &visible);
    assert!(idle.prepared.is_empty() && idle.drawn.is_empty());
    assert_eq!(done.len(), 1);
}

#[test]
fn folder_icon_batches_hold_at_most_a_hundred_and_only_platforms_that_ask_batch() {
    let sizes = |n: u64| folder_batches((0..n).map(|k| folder(k, 0)).collect()).iter().map(Vec::len).collect::<Vec<_>>();
    assert_eq!(sizes(0), Vec::<usize>::new());
    assert_eq!(sizes(1), [1]);
    assert_eq!(sizes(100), [100]);
    assert_eq!(sizes(205), [100, 100, 5]);
    assert!(batched::<Folders>(&folder(1, 0)));
    assert!(!batched::<Folders>(&IconRequest { kind: IconKind::Folder, ..folder(1, 0) }));
    assert!(!batched::<Plain>(&folder(1, 0)), "other platforms keep one request per job");
}
