//! Count the redundant stereo output allocation present at 9da40bf.
use super::decode::stereo;
use crate::test_alloc::measure;
use std::{hint::black_box, time::Instant};
fn old_stereo(bytes: &[u8], samples: usize) -> Option<Vec<i16>> {
    if samples == 0 || bytes.len() < samples * 2 {
        return None;
    }
    let channels = bytes.len() / (samples * 2);
    let pcm: Vec<i16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect();
    let mut out = Vec::with_capacity(samples * 2);
    match channels {
        1 => pcm.iter().for_each(|&x| out.extend([x, x])),
        2 => out = pcm,
        // 5.1 in WAVE order: front left and right, centre, LFE, back left and right. The gain keeps
        // the loudest case (everything at full scale in one place) from clipping.
        6 => {
            const SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;
            const GAIN: f32 = 1.0 / (1.0 + 2.0 * SIDE);
            for f in pcm.as_chunks::<6>().0 {
                let (l, r, c, back_l, back_r) = (f[0], f[1], f[2], f[4], f[5]);
                let mix = |front: i16, back: i16| {
                    ((f32::from(front) + SIDE * f32::from(c) + SIDE * f32::from(back)) * GAIN)
                        as i16
                };
                out.extend([mix(l, back_l), mix(r, back_r)]);
            }
        }
        n => pcm.chunks_exact(n).for_each(|f| out.extend([f[0], f[1]])),
    }
    Some(out)
}
fn fixture(samples: usize, channels: usize) -> Vec<u8> {
    (0..samples * channels)
        .flat_map(|i| ((i as i16).wrapping_mul(127)).to_le_bytes())
        .collect()
}
#[test]
fn stereo_conversion_allocates_only_its_returned_samples() {
    for samples in [1, 32, 1536, 4096] {
        let bytes = fixture(samples, 2);
        let before = measure(|| {
            black_box(old_stereo(&bytes, samples));
        });
        let after = measure(|| {
            black_box(stereo(&bytes, samples));
        });
        assert_eq!(before.allocations, 2);
        assert_eq!(after.allocations, 1);
        assert_eq!(after.reallocations, 0);
        assert_eq!(after.requested_bytes, samples * 2 * 2);
    }
}
#[test]
fn stereo_fast_return_preserves_all_channel_layouts_and_edge_inputs() {
    for channels in [1, 2, 3, 6, 8] {
        for samples in [0, 1, 32, 1536] {
            let mut bytes = fixture(samples, channels);
            for tail in [0x81, 0, 0xfe, 0x11] {
                assert_eq!(stereo(&bytes, samples), old_stereo(&bytes, samples));
                bytes.push(tail);
            }
        }
    }
}
#[test]
#[ignore = "manual stereo allocation/timing comparison; run release, one thread"]
fn measure_stereo_conversion() {
    let samples = 1536;
    let bytes = fixture(samples, 2);
    let before = measure(|| {
        black_box(old_stereo(&bytes, samples));
    });
    let after = measure(|| {
        black_box(stereo(&bytes, samples));
    });
    for old in [true, false] {
        for _ in 0..100 {
            black_box(if old {
                old_stereo(black_box(&bytes), samples)
            } else {
                stereo(black_box(&bytes), samples)
            });
        }
    }
    let mut times = [Vec::new(), Vec::new()];
    for pair in 0..9 {
        for index in if pair % 2 == 0 { [0, 1] } else { [1, 0] } {
            let start = Instant::now();
            for _ in 0..10000 {
                black_box(if index == 0 {
                    old_stereo(black_box(&bytes), samples)
                } else {
                    stereo(black_box(&bytes), samples)
                });
            }
            times[index].push(start.elapsed().as_secs_f64() * 1e6 / 10000.0);
        }
    }
    for times in &mut times {
        times.sort_by(f64::total_cmp);
    }
    println!(
        "samples={samples} before={before:?} after={after:?} median_us={:.6}/{:.6} p95_us={:.6}/{:.6}",
        times[0][4], times[1][4], times[0][8], times[1][8]
    );
}
