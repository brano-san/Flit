use super::*;
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::path::FsPath;
use crate::sort::{self, CaseMode, GroupField, GroupSpec, SortKeys, ViewSpec};
use std::sync::Arc;
use std::time::Duration;

fn view(names: &[&str], group: GroupField) -> DirView {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta::default());
        keys.push(listing.name(i));
    }
    let view = ViewSpec { group: GroupSpec { field: group, ..Default::default() }, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    DirView::new(Arc::new(DirSnapshot { gen: 1, path: FsPath::parse(r"C:\t").unwrap(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }))
}

fn grouped() -> DirView {
    view(&["a1", "a2", "a3", "q1", "q2"], GroupField::Name)
}

#[test]
fn rows_without_groups_stack_like_the_details_list() {
    let v = view(&["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"], GroupField::None);
    let f = Flow::new(FlowParams { across: false, per: 1, cell: (500.0, 24.0), head: 24.0 }, Some(&v));
    assert_eq!(f.extent(), 240.0);
    assert_eq!(f.cell(3), Rect::new(0.0, 72.0, 500.0, 24.0));
    assert_eq!(f.lines_in(30.0, 100.0), 1..5);
    assert_eq!(f.line_at(50.0, 499.0), Some(2));
    assert_eq!(f.line_at(250.0, 10.0), None);
    assert_eq!((f.step(4, 1), f.step(0, -1)), (Some(5), None));
    let empty = Flow::new(FlowParams::default(), Some(&view(&[], GroupField::None)));
    assert_eq!((empty.extent(), empty.lines_in(0.0, 100.0)), (0.0, 0..0));
}

#[test]
fn tiles_wrap_into_bands_under_full_width_group_headers() {
    let v = grouped();
    let f = Flow::new(FlowParams { across: false, per: 2, cell: (100.0, 50.0), head: 30.0 }, Some(&v));
    assert_eq!(f.extent(), 210.0);
    let cells: Vec<Rect> = (0..7).map(|n| f.cell(n)).collect();
    assert_eq!(cells[0], Rect::new(0.0, 0.0, 200.0, 30.0));
    assert_eq!(cells[2], Rect::new(100.0, 30.0, 100.0, 50.0));
    assert_eq!(cells[3], Rect::new(0.0, 80.0, 100.0, 50.0));
    assert_eq!(cells[4], Rect::new(0.0, 130.0, 200.0, 30.0));
    assert_eq!(cells[6], Rect::new(100.0, 160.0, 100.0, 50.0));
    assert_eq!((f.line_at(10.0, 150.0), f.line_at(40.0, 120.0), f.line_at(85.0, 150.0)), (Some(0), Some(2), None));
    assert_eq!(f.lines_in(0.0, 60.0), 0..3);
    assert_eq!(f.lines_in(100.0, 140.0), 3..5);
    assert_eq!(f.nearest(500.0, 0.0), Some(5));
}

#[test]
fn band_steps_keep_the_slot_and_skip_group_headers() {
    let v = grouped();
    let f = Flow::new(FlowParams { across: false, per: 2, cell: (100.0, 50.0), head: 30.0 }, Some(&v));
    assert_eq!(f.step(1, 1), Some(3));
    assert_eq!(f.step(2, 1), Some(3), "a short last band takes its last tile");
    assert_eq!(f.step(3, 1), Some(5));
    assert_eq!(f.step(6, -1), Some(3));
    assert_eq!(f.step(1, -1), None);
    assert_eq!(f.step(5, 1), None);
}

#[test]
fn list_columns_give_group_headers_one_slot_and_start_each_group_in_a_new_column() {
    let v = grouped();
    let f = Flow::new(FlowParams { across: true, per: 3, cell: (120.0, 20.0), head: 20.0 }, Some(&v));
    assert_eq!(f.extent(), 360.0);
    assert_eq!(f.cell(0), Rect::new(0.0, 0.0, 120.0, 20.0));
    assert_eq!(f.cell(3), Rect::new(120.0, 0.0, 120.0, 20.0));
    assert_eq!(f.cell(6), Rect::new(240.0, 40.0, 120.0, 20.0));
    assert_eq!((f.line_at(130.0, 5.0), f.line_at(130.0, 25.0)), (Some(3), None));
    assert_eq!(f.step(1, 1), Some(3));
    assert_eq!(f.step(3, 1), Some(5));
    assert_eq!(f.lines_in(100.0, 250.0), 0..7);
}
