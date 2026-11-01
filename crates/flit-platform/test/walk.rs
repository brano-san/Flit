use super::*;

struct Noise(u64);

impl Noise {
    fn next(&mut self, spread: f64) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        1.0 + spread * ((self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0)
    }
}

fn solid(peak: usize) -> impl Fn(usize) -> (f64, f64) {
    move |n| {
        let (n, p) = (n as f64, peak as f64);
        let rate = if n <= p {
            100_000.0 * n / p
        } else {
            100_000.0 * (p / n).sqrt()
        };
        (rate, n / rate)
    }
}

fn run(
    mut t: Throttle,
    model: impl Fn(usize) -> (f64, f64),
    windows: usize,
    seed: u64,
) -> Vec<usize> {
    let mut noise = Noise(seed);
    (0..windows)
        .map(|_| {
            let (rate, latency) = model(t.threads());
            t.observe(rate * noise.next(0.10), latency * noise.next(0.10))
        })
        .collect()
}

#[test]
fn the_throttle_finds_the_peak_and_stays_near_it() {
    for peak in [1, 4, 16, 64] {
        for seed in 1..=20 {
            let trail = run(
                Throttle::with_cores(Media::Solid, 32, 64 << 10),
                solid(peak),
                200,
                seed * 7919,
            );
            let settled = &trail[80..];
            let (lo, hi) = (
                settled.iter().min().copied().unwrap(),
                settled.iter().max().copied().unwrap(),
            );
            let rate = |n: usize| solid(peak)(n).0;
            let mean = settled.iter().map(|&n| rate(n)).sum::<f64>() / settled.len() as f64;
            assert!(
                mean >= 0.9 * rate(peak),
                "peak {peak} seed {seed} keeps {mean:.0}/s: {trail:?}"
            );
            assert!(
                lo >= (peak / 2).max(1) && hi <= (peak * 3 / 2).max(peak + 1),
                "peak {peak} seed {seed} swings {lo}..{hi}: {trail:?}"
            );
            let mut sorted = settled.to_vec();
            sorted.sort();
            let median = sorted[sorted.len() / 2];
            assert!(
                median >= (peak * 3 / 4).max(1) && median <= (peak * 5 / 4).max(peak + 1),
                "peak {peak} seed {seed} settles at {median}: {trail:?}"
            );
            let probes = settled.windows(2).filter(|w| w[0] != w[1]).count();
            assert!(
                probes <= settled.len() / 4,
                "peak {peak} seed {seed} moves {probes} times: {trail:?}"
            );
        }
    }
}

#[test]
fn a_single_core_with_a_spinning_disk_holds_one_or_two_threads() {
    let hdd = |n: usize| {
        let rate = match n {
            1 => 120.0,
            2 => 125.0,
            n => 250.0 / n as f64,
        };
        (rate * 60.0, 0.008 * n as f64)
    };
    for seed in 1..=20 {
        let t = Throttle::with_cores(Media::Spinning, 1, 64 << 10);
        assert_eq!(t.threads(), 1);
        let trail = run(t, hdd, 80, seed);
        assert!(
            trail.iter().all(|&n| (1..=2).contains(&n)),
            "seed {seed}: {trail:?}"
        );
    }
}

#[test]
fn a_spinning_disk_is_capped_while_it_seeks_but_not_while_it_hits_the_cache() {
    let cached = |n: usize| (50_000.0 * n as f64, 0.0001);
    let trail = run(
        Throttle::with_cores(Media::Spinning, 32, 64 << 10),
        cached,
        30,
        3,
    );
    assert!(trail.iter().any(|&n| n > SPINNING_CEILING), "{trail:?}");
    let seeking = |n: usize| (500.0 * n as f64, 0.008);
    let trail = run(
        Throttle::with_cores(Media::Spinning, 32, 64 << 10),
        seeking,
        30,
        3,
    );
    assert!(trail.iter().all(|&n| n <= SPINNING_CEILING), "{trail:?}");
}

#[test]
fn rising_listing_latency_halves_the_threads() {
    let mut t = Throttle::with_cores(Media::Solid, 32, 64 << 10);
    let model = solid(16);
    let mut noise = Noise(5);
    for _ in 0..120 {
        let (rate, latency) = model(t.threads());
        t.observe(rate * noise.next(0.03), latency);
    }
    let before = t.threads();
    let (rate, latency) = model(before);
    assert_eq!(
        t.observe(rate, latency * 4.0),
        before,
        "one slow window is not contention"
    );
    for _ in 0..3 {
        let (rate, latency) = model(t.threads());
        t.observe(rate, latency);
    }
    let busy_disk = |n: usize| (20_000.0, n as f64 / 20_000.0 * 2.0);
    let trail = run(t, busy_disk, 40, 9);
    assert!(
        trail.iter().any(|&n| n <= before / 2),
        "{before} -> {trail:?}"
    );
    assert!(
        trail[30..].iter().filter(|&&n| n <= before / 2).count() >= 8,
        "{before} -> {trail:?}"
    );
}

#[test]
fn the_ceiling_respects_the_buffer_budget_and_media() {
    assert_eq!(
        Throttle::with_cores(Media::Solid, 196, 64 << 10).ceiling,
        64
    );
    assert_eq!(Throttle::with_cores(Media::Solid, 196, 1 << 20).ceiling, 4);
    assert_eq!(
        Throttle::with_cores(Media::Network, 196, 64 << 10).threads(),
        1
    );
    assert_eq!(
        Throttle::with_cores(Media::Unknown, 196, 64 << 10).ceiling,
        MODEST_CEILING
    );
    let mut fixed = Throttle::fixed(16);
    assert_eq!(fixed.observe(1.0, 1.0), 16);
}

struct Leaf(usize);

#[test]
fn the_walk_lists_every_directory_once_and_stops_when_asked() {
    let list = |&(depth, _): &(u32, u32), subdirs: &mut Vec<(u32, u32)>| {
        if depth < 4 {
            subdirs.extend((0..6).map(|i| (depth + 1, i)));
        }
        std::thread::sleep(Duration::from_micros(50));
        Leaf(7)
    };
    let mut dirs = 0;
    let t = walk(
        (0u32, 0u32),
        Throttle::with_cores(Media::Solid, 4, 64 << 10),
        list,
        |b, done, total| {
            dirs += 1;
            assert_eq!(b.0, 7);
            assert!(done <= total);
            true
        },
    );
    assert_eq!(dirs, 1 + 6 + 36 + 216 + 1296);
    assert!(t.threads() >= 1);
    let mut seen = 0;
    walk((0u32, 0u32), Throttle::fixed(8), list, |_, _, _| {
        seen += 1;
        seen < 10
    });
    assert!(seen < 1555, "{seen}");
}
