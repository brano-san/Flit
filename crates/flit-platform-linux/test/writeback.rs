use super::*;
use std::cell::{Cell, RefCell};

#[test]
fn slow_destinations_submit_two_windows_and_report_only_the_waited_one() {
    let now = Instant::now();
    let time = Cell::new(now);
    let log = RefCell::new(Vec::new());
    let write = |kind, from, bytes| {
        log.borrow_mut().push((kind, from, bytes));
        Ok(())
    };
    let mut ranges = Ranges::new(4 * MIB);
    assert_eq!(ranges.room(), 4 * MIB);
    ranges.add(0, 4 * MIB);
    assert_eq!(ranges.submit(write, || time.get()).unwrap(), None);
    ranges.add(4 * MIB as u64, 4 * MIB);
    time.set(now + Duration::from_millis(125));
    assert_eq!(ranges.submit(write, || time.get()).unwrap(), Some(4 * MIB as u64));
    assert_eq!(*log.borrow(), [(Flush::Write, 0, 4 * MIB), (Flush::Write, 4 * MIB as u64, 4 * MIB), (Flush::Wait, 0, 4 * MIB)]);
    assert_eq!(ranges.room(), 8 * MIB);
    time.set(now + Duration::from_secs(10));
    assert_eq!(ranges.wait(write, || time.get()).unwrap(), Some(8 * MIB as u64));
    assert_eq!(ranges.room(), MIB);
}

#[test]
fn internal_ssd_and_network_mounts_keep_normal_writes() {
    let mounts = crate::mounts::parse(b"20 1 8:1 / /media/stick rw - ext4 /dev/sda1 rw\n21 1 8:2 / /data rw - ext4 /dev/sda2 rw\n22 1 8:3 / /ssd rw - ext4 /dev/sda3 rw\n23 1 0:42 / /media/share rw - cifs //nas/share rw\n");
    let spinning = |dev, _: Option<&Mount>| dev == (8, 2);
    assert!(slow(&mounts, (8, 1), spinning));
    assert!(slow(&mounts, (8, 2), spinning));
    assert!(!slow(&mounts, (8, 3), spinning));
    assert!(!slow(&mounts, (0, 42), |_, _| true));
}

#[test]
fn unsupported_ranges_fall_back_to_a_single_volume_flush() {
    let mut ranges = Ranges::new(4 * MIB);
    ranges.add(15, 64);
    assert_eq!(ranges.submit(|_, _, _| Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)), Instant::now).unwrap(), Some(79));
    assert_eq!(ranges.size(), None);
    ranges.add(79, 64);
    assert_eq!(ranges.submit(|_, _, _| panic!("unsupported range syscall retried"), Instant::now).unwrap(), None);
}

#[test]
fn a_partial_window_does_not_change_the_adaptive_window() {
    let now = Instant::now();
    let mut ranges = Ranges::new(4 * MIB);
    ranges.add(0, 512);
    assert_eq!(ranges.submit(|_, _, _| Ok(()), || now).unwrap(), None);
    assert_eq!(ranges.wait(|_, _, _| Ok(()), || now + Duration::from_secs(3)).unwrap(), Some(512));
    assert_eq!(ranges.size(), Some(4 * MIB));
}

#[test]
fn small_copies_share_a_volume_flush_and_only_report_bytes_after_it() {
    let temp = crate::temp::Temp::new("writeback-batch");
    let fd = std::fs::File::open(temp.path()).unwrap();
    let dest = FsPath::parse("/copy/dest").unwrap();
    let mut volumes = Volumes::default();
    volumes.devices.insert(1, Device { fs: None, through: Some(true), sync: None });
    volumes.devices.insert(2, Device { fs: None, through: Some(true), sync: None });

    use std::os::fd::AsFd;
    for _ in 0..1024 {
        assert_eq!(volumes.open(1, fd.as_fd(), &dest, 16 * 1024).unwrap(), CopyMode::Deferred);
        volumes.defer(1, 16 * 1024);
    }
    assert_eq!(volumes.open(1, fd.as_fd(), &dest, START as u64).unwrap(), CopyMode::Windowed(START));
    assert_eq!(volumes.open(2, fd.as_fd(), &dest, 64).unwrap(), CopyMode::Deferred);
    volumes.defer(2, 64);
    let bad_fd = volumes.devices[&2].sync.as_ref().unwrap().fd.as_raw_fd();
    let (mut calls, expected) = (0, 16 * MIB as u64);
    let (confirmed, failures) = volumes.finish_with(|fd| {
        calls += 1;
        if fd == bad_fd {
            Err(io::Error::other("flush failed"))
        } else {
            Ok(())
        }
    });
    assert_eq!((calls, confirmed, failures.len()), (2, expected, 1));
    assert_eq!(volumes.finish_with(|_| panic!("volume flushed twice")).0, 0);
}
