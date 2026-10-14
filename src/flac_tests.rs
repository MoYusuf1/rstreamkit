//! Exact FLAC output and allocation measurements for scratch-buffer changes.
use super::flac_frame;
use crate::test_alloc::measure;
use std::hint::black_box;

fn input(pattern: usize, samples: usize) -> Vec<i16> {
    let mut state = 0x12345678u32;
    (0..samples * 2)
        .map(|i| match pattern {
            0 => 321,
            1 => (i as i16).wrapping_mul(7),
            2 => {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as i16
            }
            3 => {
                if i % 3 == 0 {
                    i16::MIN
                } else {
                    i16::MAX
                }
            }
            _ => {
                if i % 2 == 0 {
                    0
                } else {
                    (i as i16).wrapping_mul(31)
                }
            }
        })
        .collect()
}

#[test]
#[ignore = "manual allocation and complete-output baseline; run release with nocapture"]
fn measure_flac_scratch() {
    for pattern in 0..5 {
        let pcm = input(pattern, 1536);
        let allocation = measure(|| {
            black_box(flac_frame(&pcm, 128));
        });
        let (hash, bytes) = complete_output(pattern);
        println!(
            "pattern={pattern} bytes={bytes} hash={hash:016x} allocations={} reallocations={} requested_bytes={}",
            allocation.allocations, allocation.reallocations, allocation.requested_bytes
        );
    }
}

fn complete_output(pattern: usize) -> (u64, usize) {
    let mut hash = 0xcbf29ce484222325u64;
    let mut bytes = 0;
    for samples in [1, 32, 192, 1152, 1536, 4096] {
        let pcm = input(pattern, samples);
        for number in [0, 127, 128, 65535, 0x7fffffff] {
            let output = flac_frame(&pcm, number);
            bytes += output.len();
            for byte in output {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
    }
    (hash, bytes)
}

#[test]
fn flac_complete_bytes_match_before_scratch_reuse() {
    let expected = [
        (0xd6e6391715ee7cdb, 488),
        (0x5acd272c87277612, 40093),
        (0x3c9c966c165bcdd8, 140548),
        (0xa2c57e4a695fe122, 140548),
        (0xce6c0692c8f86c38, 38428),
    ];
    for (pattern, expected) in expected.into_iter().enumerate() {
        assert_eq!(complete_output(pattern), expected, "pattern {pattern}");
    }
}

#[test]
fn flac_scratch_does_not_allocate_again_for_the_second_channel() {
    for pattern in 0..5 {
        let pcm = input(pattern, 1536);
        let stats = measure(|| {
            black_box(flac_frame(&pcm, 128));
        });
        assert!(stats.allocations <= 4, "pattern {pattern}: {stats:?}");
        assert!(
            stats.requested_bytes <= 18461,
            "pattern {pattern}: {stats:?}"
        );
    }
}
