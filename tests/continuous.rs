use rstreamkit::{Continuous, Transmuxer, ts};
#[test]
fn continuous_preserves_every_picture_with_arbitrary_network_chunks() {
    let input = include_bytes!("fixtures/bbb_480p.ts");
    let expected = ts::demux(input).unwrap().video.len();
    for size in [1, 187, 188, 513, 16384, input.len()] {
        let mut stream = Continuous::default();
        let mut pictures = 0;
        let mut inits = 0;
        let mut consume = |out: rstreamkit::Output| {
            inits += usize::from(out.init.is_some());
            for f in out.fragments {
                let header = f.moof.windows(4).position(|b| b == b"tfhd").unwrap();
                if u32::from_be_bytes(f.moof[header + 8..header + 12].try_into().unwrap()) != 1 {
                    continue;
                }
                if let Some(at) = f.moof.windows(4).position(|b| b == b"trun") {
                    pictures +=
                        u32::from_be_bytes(f.moof[at + 8..at + 12].try_into().unwrap()) as usize;
                }
            }
        };
        for bytes in input.chunks(size) {
            for out in stream.feed(bytes).unwrap() {
                consume(out);
            }
        }
        consume(stream.finish().unwrap());
        assert_eq!(pictures, expected, "chunk size {size}");
        assert_eq!(inits, 1);
    }
}
#[test]
fn continuous_caps_input_without_a_keyframe() {
    let input = include_bytes!("fixtures/bbb_480p.ts");
    assert!(Continuous::new(4096).feed(input).is_err());
    // A segmented pipeline remains usable after an independent continuous input fails.
    assert!(Transmuxer::default().push(input).is_ok());
}

#[test]
fn endless_garbage_fails_before_waiting_for_a_keyframe() {
    assert!(Continuous::default().feed(&vec![0; 16384]).is_err());
}
