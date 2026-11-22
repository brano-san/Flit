use super::*;

#[test]
fn a_waiting_copy_uses_the_new_rate_at_the_next_slice() {
    let mut bucket = Bucket::default();
    let now = Instant::now();
    assert_eq!(bucket.charge(now, 1, 300_000), Duration::from_millis(200));
    assert_eq!(bucket.charge(now + Duration::from_millis(100), 2, 0), Duration::from_millis(50));
    assert_eq!(bucket.charge(now + Duration::from_millis(150), 2, 0), Duration::ZERO);
}

#[test]
fn unlimited_discards_debt_and_does_not_bank_a_burst() {
    let mut bucket = Bucket::default();
    let now = Instant::now();
    assert_eq!(bucket.charge(now, 3, 600_000), Duration::from_millis(100));
    assert_eq!(bucket.charge(now, 0, 1_000_000), Duration::ZERO);
    assert_eq!(bucket.charge(now, 3, 300_000), Duration::ZERO);
}
