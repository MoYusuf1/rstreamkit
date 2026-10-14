//! Test-only comparison against the collected scanner from cb6e2a3.
use super::nal_units;
use crate::test_alloc::{Stats, measure};
use std::{hint::black_box, time::Instant};
/// Splits a PES payload of Annex-B data into NAL units (without start codes).
fn collected_nal_units(es: &[u8]) -> Vec<&[u8]> {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGHS: u64 = 0x8080_8080_8080_8080;
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 <= es.len() {
        // A start code begins with a zero byte, and compressed pictures have few: look at eight
        // bytes at a time and go straight to the first zero (the lowest flagged byte is exact).
        if let Some(word) = es.get(i..i + 8) {
            let w = u64::from_le_bytes(word.try_into().unwrap());
            let zeros = w.wrapping_sub(ONES) & !w & HIGHS;
            if zeros == 0 {
                i += 8;
                continue;
            }
            i += zeros.trailing_zeros() as usize / 8;
            if i + 3 > es.len() {
                break;
            }
        }
        if es[i] == 0 && es[i + 1] == 0 && es[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = vec![];
    for (n, &s) in starts.iter().enumerate() {
        let mut e = starts.get(n + 1).map_or(es.len(), |&next| next - 3);
        // A 4-byte start code leaves one extra zero on the previous NAL; trailing zeros are never payload.
        while e > s && es[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            out.push(&es[s..e]);
        }
    }
    out
}

fn fixture(count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for i in 0..count {
        bytes.extend_from_slice(if i % 2 == 0 {
            &[0, 0, 1]
        } else {
            &[0, 0, 0, 1]
        });
        bytes.push(0x65);
        // Escaped payload, with zeros that must not be read as start codes.
        for _ in 0..16 {
            bytes.extend_from_slice(&[0, 0, 3, 1, 0x91, 0x83, 0x17, 0x23]);
        }
    }
    bytes
}
fn scan(bytes: &[u8], collected: bool) -> usize {
    if collected {
        collected_nal_units(black_box(bytes))
            .into_iter()
            .map(|nal| black_box(nal).len())
            .sum()
    } else {
        nal_units(black_box(bytes))
            .map(|nal| black_box(nal).len())
            .sum()
    }
}
#[test]
fn lazy_scan_matches_collected_scan_without_heap_storage() {
    for count in [0, 1, 128, 4096] {
        let bytes = fixture(count);
        assert!(nal_units(&bytes).eq(collected_nal_units(&bytes)));
        let stats = measure(|| {
            assert_eq!(scan(&bytes, false), count * 129);
        });
        assert_eq!(stats, Stats::default());
    }
}
#[test]
#[ignore = "manual allocation and timing comparison; run release, one test thread"]
fn measure_nal_scan() {
    for count in [1, 128, 4096] {
        let bytes = fixture(count);
        let before = measure(|| {
            black_box(scan(&bytes, true));
        });
        let after = measure(|| {
            black_box(scan(&bytes, false));
        });
        let rounds = 100.max(100_000 / count);
        for collected in [true, false] {
            for _ in 0..20 {
                black_box(scan(&bytes, collected));
            }
        }
        let mut samples = [Vec::new(), Vec::new()];
        for pair in 0..9 {
            for index in if pair % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                for _ in 0..rounds {
                    black_box(scan(&bytes, index == 0));
                }
                samples[index].push(start.elapsed().as_secs_f64() * 1e6 / rounds as f64);
            }
        }
        for times in &mut samples {
            times.sort_by(f64::total_cmp);
        }
        println!(
            "nals={count} bytes={} before={before:?} after={after:?} median_us={:.3}/{:.3} p95_us={:.3}/{:.3}",
            bytes.len(),
            samples[0][4],
            samples[1][4],
            samples[0][8],
            samples[1][8]
        );
    }
}
