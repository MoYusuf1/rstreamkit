use rstreamkit::{Continuous, Output, ts};

fn encoded_audio(d: &ts::Demuxed) -> Vec<Vec<u8>> {
    d.audio
        .iter()
        .map(|s| s.data.as_slice())
        .chain(d.sound.iter().map(|s| s.data.as_slice()))
        .map(|data| data.to_vec())
        .collect()
}

#[test]
fn continuous_keeps_coded_audio_in_order_across_short_clock_resets() {
    for clip in [
        include_bytes!("fixtures/bbb_480p.ts").as_slice(),
        include_bytes!("fixtures/h264_ac3_51.ts").as_slice(),
        include_bytes!("fixtures/h264_ac3.ts").as_slice(),
        include_bytes!("fixtures/h264_ac3_st.ts").as_slice(),
        include_bytes!("fixtures/h264_eac3_51.ts").as_slice(),
        include_bytes!("fixtures/h264_mp2.ts").as_slice(),
    ] {
        let original = encoded_audio(&ts::demux(clip).unwrap());
        let expected: Vec<_> = (0..3).flat_map(|_| original.iter().cloned()).collect();
        let input = clip.repeat(3);
        for chunk_size in [187, 16384, input.len()] {
            let mut demuxer = ts::Demuxer::new(vec![0; 8]);
            let mut actual = Vec::new();
            for chunk in input.chunks(chunk_size) {
                demuxer.feed(chunk);
                while let Some(fragment) = demuxer.take_fragment().unwrap() {
                    actual.extend(encoded_audio(&fragment));
                }
            }
            actual.extend(encoded_audio(&demuxer.finish().unwrap()));
            assert_eq!(
                actual.len(),
                expected.len(),
                "chunk {chunk_size}: coded frames lost"
            );
            if actual != expected {
                let at = actual
                    .iter()
                    .zip(&expected)
                    .position(|(a, b)| a != b)
                    .unwrap();
                println!(
                    "first mismatch frame {at}, actual source {:?}, expected source {:?}",
                    expected.iter().position(|f| f == &actual[at]),
                    expected.iter().position(|f| f == &expected[at])
                );
            }
            assert!(
                actual == expected,
                "chunk {chunk_size}: coded audio reordered across reset"
            );
        }
    }
}

fn track_times(out: Output, ends: &mut [Option<u64>; 2], pictures: &mut u32) {
    for f in out.fragments {
        let word = |at: usize| u32::from_be_bytes(f.moof[at..at + 4].try_into().unwrap());
        let tfhd = f.moof.windows(4).position(|s| s == b"tfhd").unwrap();
        let tfdt = f.moof.windows(4).position(|s| s == b"tfdt").unwrap();
        let trun = f.moof.windows(4).position(|s| s == b"trun").unwrap();
        let track = word(tfhd + 8) as usize - 1;
        let base = u64::from_be_bytes(f.moof[tfdt + 8..tfdt + 16].try_into().unwrap());
        let count = word(trun + 8);
        if track == 0 {
            *pictures += count;
        }
        if let Some(previous) = ends[track] {
            assert_eq!(base, previous, "track {} has an overlap or hole", track + 1);
        }
        let stride = if track == 0 { 16 } else { 12 };
        let duration = (0..count as usize)
            .map(|i| u64::from(word(trun + 16 + i * stride)))
            .sum::<u64>();
        ends[track] = Some(base + duration);
    }
}

#[test]
fn continuous_short_restarts_keep_every_picture_on_contiguous_clocks() {
    for clip in [
        include_bytes!("fixtures/bbb_480p.ts").as_slice(),
        include_bytes!("fixtures/h264_ac3_51.ts").as_slice(),
        include_bytes!("fixtures/h264_ac3.ts").as_slice(),
        include_bytes!("fixtures/h264_ac3_st.ts").as_slice(),
        include_bytes!("fixtures/h264_eac3_51.ts").as_slice(),
        include_bytes!("fixtures/h264_mp2.ts").as_slice(),
    ] {
        let expected = ts::demux(clip).unwrap().video.len() as u32 * 3;
        let input = clip.repeat(3);
        for chunk_size in [187, 16384, input.len()] {
            let mut stream = Continuous::default().decode_sound(true);
            let (mut ends, mut pictures) = ([None; 2], 0);
            for chunk in input.chunks(chunk_size) {
                for out in stream.feed(chunk).unwrap() {
                    track_times(out, &mut ends, &mut pictures);
                }
            }
            track_times(stream.finish().unwrap(), &mut ends, &mut pictures);
            assert_eq!(pictures, expected, "chunk {chunk_size}: pictures lost");
        }
    }
}
