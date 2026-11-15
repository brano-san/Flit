use super::*;

fn pinches(out: &[Touch]) -> f32 {
    out.iter().map(|t| if let Touch::Pinch(d) = t { *d } else { 0.0 }).sum()
}

#[test]
fn one_finger_passes_through_as_a_pointer_and_never_pinches() {
    let mut f = Fingers::default();
    let mut out = vec![f.press(7, (100.0, 100.0))];
    for k in 1..=20 {
        out.push(f.moved(7, (100.0 + 10.0 * k as f64, 100.0 + 5.0 * k as f64)));
    }
    out.push(f.release(7));
    assert!(out.iter().all(|t| *t == Touch::Pass), "{out:?}");
    assert_eq!(f.lost(7), Touch::Skip, "the leave after a release is not a cancel");
}

#[test]
fn spreading_two_fingers_zooms_in_by_the_ratio_of_their_distance() {
    let mut f = Fingers::default();
    assert_eq!(f.press(1, (100.0, 100.0)), Touch::Pass);
    assert_eq!(f.press(2, (200.0, 100.0)), Touch::Cancel, "the second finger ends the tap or pan of the first");
    let out = vec![f.moved(2, (225.0, 100.0)), f.moved(2, (250.0, 100.0))];
    assert!((pinches(&out) - 0.5).abs() < 1e-5, "100 -> 150 px is a 1.5x pinch: {out:?}");
    assert!(out.iter().all(|t| matches!(t, Touch::Pinch(d) if *d > 0.0)));
    let out = vec![f.moved(1, (150.0, 100.0)), f.moved(1, (175.0, 100.0))];
    assert!((pinches(&out) + 0.75).abs() < 1e-5, "moving either finger counts and pinching closer is negative: {out:?}");
}

#[test]
fn the_pinch_sum_does_not_depend_on_how_many_moves_arrive() {
    let run = |steps: u32| {
        let mut f = Fingers::default();
        f.press(1, (0.0, 0.0));
        f.press(2, (80.0, 60.0));
        let out: Vec<Touch> = (1..=steps).map(|k| f.moved(2, (80.0 + 80.0 * k as f64 / steps as f64, 60.0 + 60.0 * k as f64 / steps as f64))).collect();
        pinches(&out)
    };
    assert!((run(1) - 1.0).abs() < 1e-5);
    assert!((run(37) - 1.0).abs() < 1e-4);
}

#[test]
fn lifting_a_finger_or_a_third_finger_ends_the_pinch_until_all_are_up() {
    let mut f = Fingers::default();
    f.press(1, (0.0, 0.0));
    f.press(2, (100.0, 0.0));
    assert!(matches!(f.moved(2, (120.0, 0.0)), Touch::Pinch(_)));
    assert_eq!(f.press(3, (50.0, 50.0)), Touch::Skip);
    assert_eq!(f.moved(2, (200.0, 0.0)), Touch::Skip, "a third finger ends the gesture");
    assert_eq!(f.release(3), Touch::Skip);
    assert_eq!(f.moved(1, (10.0, 0.0)), Touch::Skip);
    assert_eq!(f.release(1), Touch::Skip, "the first finger's lift is not a click");
    assert_eq!(f.moved(2, (220.0, 0.0)), Touch::Skip, "the finger left behind does not pan");
    assert_eq!(f.release(2), Touch::Skip);
    assert_eq!(f.press(4, (10.0, 10.0)), Touch::Pass, "a fresh touch starts over");

    let mut f = Fingers::default();
    f.press(1, (0.0, 0.0));
    f.press(2, (100.0, 0.0));
    assert_eq!(f.release(2), Touch::Skip);
    assert_eq!(f.moved(1, (40.0, 0.0)), Touch::Skip);
    assert_eq!(f.release(1), Touch::Skip);
}

#[test]
fn a_lost_touch_cancels_and_odd_input_never_breaks() {
    let mut f = Fingers::default();
    f.press(5, (10.0, 10.0));
    assert_eq!(f.lost(5), Touch::Cancel, "a leave without a release is a cancel");
    assert_eq!(f.release(5), Touch::Skip, "a stray release after it is ignored");
    assert_eq!(f.moved(9, (1.0, 1.0)), Touch::Skip, "an unknown finger is ignored");
    assert_eq!(f.release(9), Touch::Skip);

    let mut f = Fingers::default();
    f.press(1, (50.0, 50.0));
    f.press(2, (50.0, 50.0));
    assert_eq!(f.moved(2, (50.0, 50.0)), Touch::Skip, "two fingers on one spot give no division by zero");
    assert_eq!(f.moved(2, (f64::NAN, 50.0)), Touch::Skip);
    assert!(matches!(f.moved(2, (150.0, 50.0)), Touch::Skip));
    assert!(matches!(f.moved(2, (250.0, 50.0)), Touch::Pinch(d) if d > 0.0 && d.is_finite()));

    let mut f = Fingers::default();
    f.press(1, (0.0, 0.0));
    f.press(2, (100.0, 0.0));
    f.reset();
    assert_eq!(f.press(1, (0.0, 0.0)), Touch::Pass, "losing focus forgets every finger");
}
