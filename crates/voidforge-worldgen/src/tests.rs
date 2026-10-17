//! The sampler's own arithmetic. The defaults are tested on generated worlds in
//! `tests/worldgen.rs`.

use super::*;

#[test]
fn the_rng_stream_is_pinned() {
    // ChaCha8 from seed 42: if a rand_chacha upgrade changed the stream, every world would
    // change; this fails first. Values recorded at rand_chacha 0.9.0 (D-031).
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    let got: Vec<u64> = (0..3).map(|_| rng.next_u64()).collect();
    assert_eq!(got, PINNED);
}

const PINNED: [u64; 3] = [
    12_578_764_544_318_200_737,
    17_529_487_244_874_322_312,
    7_886_285_670_807_131_020,
];

#[test]
fn below_stays_in_range_and_reaches_every_value() {
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let mut seen = [0u32; 7];
    for _ in 0..7_000 {
        let v = below(&mut rng, 7);
        seen[v as usize] += 1;
    }
    // Each of 7 values about 1,000 times: well within 800..1,200 for an unbiased draw.
    assert!(seen.iter().all(|&n| (800..1_200).contains(&n)), "{seen:?}");
    assert_eq!(below(&mut rng, 1), 0);
}

#[test]
fn below_rejects_the_partial_top_range() {
    // n = 2^63 + 1: the accept bound is one full multiple of n, so about half the draws are
    // rejected and every returned value is still below n.
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    let n = (1u64 << 63) + 1;
    for _ in 0..100 {
        assert!(below(&mut rng, n) < n);
    }
}
