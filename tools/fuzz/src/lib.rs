//! Development-only targets shared by seeded smoke runs and cargo-fuzz.
use rstreamkit::{Continuous, Transmuxer, cmaf, hls, mkv, mp4};

pub const MAX_INPUT: usize = 1 << 20;

pub fn exercise(target: &str, data: &[u8]) {
    if data.len() > MAX_INPUT {
        return;
    }
    match target {
        "hls" => {
            let _ = hls::parse(data);
        }
        "ts" => {
            let tx = Transmuxer::default().decode_sound(false);
            let mut segment = tx.segment(data.len());
            let chunk = 1 + usize::from(data.first().copied().unwrap_or(0));
            for part in data.chunks(chunk) {
                segment.feed(part);
            }
            let _ = Transmuxer::default().decode_sound(false).finish(segment);
            let mut stream = Continuous::new(MAX_INPUT).decode_sound(false);
            for part in data.chunks(chunk) {
                if stream.feed(part).is_err() {
                    break;
                }
            }
            let _ = stream.finish();
        }
        "mp4" => {
            let _ = mp4::box_header(data);
            let _ = mp4::find_moov(data, 0);
            let _ = mp4::find_moov(data, u64::MAX);
            if let Ok(parsed) = mp4::parse(data) {
                for time in [i64::MIN, 0, parsed.duration, i64::MAX] {
                    let mut cursor = parsed.index.cursor(time);
                    let _ = parsed.index.range(&cursor, MAX_INPUT as u64);
                    let _ = parsed.index.frames(&mut cursor, 0, data);
                    let _ = parsed.index.done(&cursor);
                }
            }
        }
        "mkv" => {
            let mut probe = mkv::Probe::new(data.len() as u64);
            let mut at: usize = 0;
            let mut len = data.len();
            for _ in 0..32 {
                let Some(bytes) = data.get(at..at.saturating_add(len).min(data.len())) else {
                    break;
                };
                match probe.feed(at as u64, bytes) {
                    Ok(mkv::Step::Read(offset, count)) => {
                        let (Ok(offset), Ok(count)) =
                            (usize::try_from(offset), usize::try_from(count))
                        else {
                            break;
                        };
                        if offset == at && count == len {
                            break;
                        }
                        at = offset;
                        len = count.min(MAX_INPUT);
                    }
                    Ok(mkv::Step::Done(parsed)) => {
                        for time in [i64::MIN, 0, parsed.duration, i64::MAX] {
                            let mut cursor = parsed.index.cursor(time);
                            let _ =
                                parsed
                                    .index
                                    .range(&cursor, MAX_INPUT as u64, data.len() as u64);
                            let _ = parsed.index.frames(&mut cursor, 0, data, true);
                        }
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        "cmaf" => {
            let mut rebaser = cmaf::Rebaser::default();
            let _ = rebaser.init(data);
            let _ = cmaf::merge_init(data, data);
            if let Some(header) = data.get(..4) {
                let size = u32::from_le_bytes(header.try_into().unwrap()) as usize;
                if let Some(split) = size.checked_add(4).filter(|&n| n <= data.len()) {
                    let _ = rebaser.init(&data[4..split]);
                    for duration in [0.0, 2.0, f64::NAN, f64::INFINITY, -1.0] {
                        let mut bytes = data[split..].to_vec();
                        let original = bytes.clone();
                        if rebaser.fragment(&mut bytes, duration).is_err() {
                            assert_eq!(bytes, original, "failed rebasing modified input");
                        }
                    }
                }
            }
            let mut bytes = data.to_vec();
            let _ = cmaf::audio_fragment(&mut bytes, i64::MIN);
        }
        _ => panic!("unknown fuzz target: {target}"),
    }
}
