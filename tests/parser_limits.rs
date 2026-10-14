#![cfg(feature = "vod")]
use rstreamkit::{mkv, mp4};

#[test]
fn mp4_offsets_cannot_wrap_to_the_start_of_a_file() {
    let boxes = b"\0\0\0\x08free\0\0\0\x08moov";
    assert_eq!(mp4::find_moov(boxes, u64::MAX), mp4::Moov::Missing);
    assert_eq!(mp4::find_moov(&boxes[..8], u64::MAX), mp4::Moov::Missing);
    assert_eq!(mp4::find_moov(boxes, 42), mp4::Moov::At(50, 8));
}

#[test]
fn extreme_negative_mkv_seek_uses_the_first_cue() {
    let bytes = include_bytes!("fixtures/movie_ac3.mkv");
    let mut probe = mkv::Probe::new(bytes.len() as u64);
    let mut at = 0;
    for _ in 0..32 {
        match probe.feed(at, &bytes[at as usize..]).unwrap() {
            mkv::Step::Read(offset, _) => at = offset,
            mkv::Step::Done(parsed) => {
                let first = parsed.index.cursor(0);
                let negative = parsed.index.cursor(i64::MIN);
                assert_eq!(
                    parsed.index.range(&first, 4096, bytes.len() as u64),
                    parsed.index.range(&negative, 4096, bytes.len() as u64)
                );
                return;
            }
        }
    }
    panic!("fixture index was not found");
}

fn moov_payload(bytes: &[u8]) -> Vec<u8> {
    let mp4::Moov::At(at, size) = mp4::find_moov(bytes, 0) else {
        panic!("missing fixture index")
    };
    let moov = &bytes[at as usize..(at + size) as usize];
    let (_, _, header) = mp4::box_header(moov).unwrap();
    moov[header..].to_vec()
}

#[test]
fn oversized_timing_runs_are_rejected_before_expanding_them() {
    let original = moov_payload(include_bytes!("fixtures/movie_aac.mp4"));
    assert!(mp4::parse(&original).is_ok());
    for kind in [b"stts", b"ctts"] {
        let mut bytes = original.clone();
        let at = bytes.windows(4).position(|b| b == kind).unwrap();
        bytes[at + 12..at + 16].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(mp4::parse(&bytes).is_err());
    }
}

#[test]
fn an_impossible_edit_count_does_not_scan_missing_entries() {
    let mut bytes = moov_payload(include_bytes!("fixtures/movie_aac.mp4"));
    let at = bytes.windows(4).position(|b| b == b"elst").unwrap();
    bytes[at + 8..at + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(mp4::parse(&bytes).is_ok()); // Malformed edits use the existing zero-shift fallback.
}
