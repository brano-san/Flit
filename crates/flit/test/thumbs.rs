use super::*;
use std::time::Instant;

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-thumbs-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    Temp(d)
}

fn bytes(c: &ThumbCache) -> u64 {
    c.segs.iter().map(|s| s.1).sum()
}

fn bitmap(side: u32, seed: u8) -> Bitmap {
    Bitmap { width: side, height: side / 2, rgba: (0..side * side / 2 * 4).map(|k| (k as u8).wrapping_add(seed)).collect() }
}

#[test]
fn thumbnails_come_back_after_a_restart_and_a_torn_tail_is_ignored() {
    let d = temp("restart");
    let mut c = ThumbCache::new(d.0.clone(), 100);
    assert!(c.get(1).is_none());
    c.put(1, &bitmap(64, 1));
    c.put(2, &bitmap(256, 2));
    c.put(3, &Bitmap { width: 600, height: 1, rgba: vec![0; 2400] });
    assert_eq!(c.get(2).unwrap().rgba, bitmap(256, 2).rgba);
    drop(c);
    let mut f = OpenOptions::new().append(true).open(seg_path(&d.0, 1)).unwrap();
    f.write_all(&[7; 40]).unwrap();
    drop(f);
    let mut c = ThumbCache::new(d.0.clone(), 100);
    let b = c.get(1).unwrap();
    assert_eq!((b.width, b.height, b.rgba), (64, 32, bitmap(64, 1).rgba));
    assert!(c.get(3).is_none(), "oversized bitmaps are never stored");
    c.put(4, &bitmap(128, 4));
    drop(c);
    let mut c = ThumbCache::new(d.0.clone(), 100);
    assert_eq!(c.get(4).unwrap().rgba, bitmap(128, 4).rgba, "a record written after the torn tail is readable");
    assert!(c.get(2).is_some());
}

#[test]
fn the_oldest_thumbnails_go_first_when_the_cache_outgrows_its_limit_and_clear_removes_every_file() {
    let d = temp("limit");
    let mut c = ThumbCache::new(d.0.clone(), 1);
    let big = bitmap(512, 9);
    for k in 1..=5 {
        c.put(k, &big);
    }
    assert!((1..=4).all(|k| c.get(k).is_none()) && c.get(5).is_some());
    assert!(bytes(&c) <= 1 << 20);
    assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    c.clear();
    assert_eq!(bytes(&c), 0);
    assert!(c.get(5).is_none());
    assert_eq!(fs::read_dir(&d.0).unwrap().count(), 0);
    c.put(6, &bitmap(64, 6));
    assert!(c.get(6).is_some(), "the cache keeps working after a clear");
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn bench_thumb_cache() {
    let d = temp("bench");
    let mut c = ThumbCache::new(d.0.clone(), 500);
    for (side, n) in [(128u32, 400u64), (256, 100)] {
        let pics: Vec<Bitmap> = (0..n).map(|k| Bitmap { width: side, height: side, rgba: vec![k as u8; (side * side * 4) as usize] }).collect();
        let t = Instant::now();
        for (k, b) in pics.iter().enumerate() {
            c.put(side as u64 * 1000 + k as u64, b);
        }
        let put = t.elapsed();
        let t = Instant::now();
        let got = (0..n).filter(|k| c.get(side as u64 * 1000 + k).is_some()).count();
        let get = t.elapsed();
        let mb = (side * side * 4) as f64 * n as f64 / 1e6;
        eprintln!("BENCH thumb cache {side}px put {n} ({mb:.1} MB)  {:>8.2} ms  {:>7.0} per s", put.as_secs_f64() * 1e3, n as f64 / put.as_secs_f64());
        eprintln!("BENCH thumb cache {side}px get {got}            {:>8.2} ms  {:>7.0} per s  {:.3} ms each", get.as_secs_f64() * 1e3, got as f64 / get.as_secs_f64(), get.as_secs_f64() * 1e3 / got as f64);
        assert!(put / n as u32 <= std::time::Duration::from_millis(1), "{side}px put {:?} each", put / n as u32);
        assert!(get / got as u32 <= std::time::Duration::from_micros(500), "{side}px get {:?} each", get / got as u32);
    }
    drop(c);
    let t = Instant::now();
    let mut c = ThumbCache::new(d.0.clone(), 500);
    assert!(c.get(128_000).is_some());
    eprintln!("BENCH thumb cache index rebuild of 500 thumbnails {:>8.2} ms", t.elapsed().as_secs_f64() * 1e3);
    assert!(t.elapsed() < std::time::Duration::from_millis(30), "index rebuild took {:?}", t.elapsed());
}
