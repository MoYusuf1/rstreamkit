use rstreamkit::sound::{flac_frame, flac_streaminfo};
use std::process::Command;
#[test]
fn compressed_flac_roundtrips_silence_noise_extremes_and_predictable_audio() {
    if !Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return;
    }
    let mut pcm = vec![0i16; 1536 * 2];
    pcm.extend((0..1536).flat_map(|i| [i as i16 * 16 - 12000, 12000 - i as i16 * 16]));
    let mut rng = 1u32;
    pcm.extend((0..1536 * 2).map(|_| {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (rng >> 16) as i16
    }));
    pcm.extend((0..1536).flat_map(|i| {
        if i % 2 == 0 {
            [i16::MIN, i16::MAX]
        } else {
            [i16::MAX, i16::MIN]
        }
    }));
    let mut flac = b"fLaC".to_vec();
    flac.extend([0x80, 0, 0, 34]);
    flac.extend(flac_streaminfo(48000));
    for (i, frame) in pcm.chunks(1536 * 2).enumerate() {
        flac.extend(flac_frame(frame, i as u32));
    }
    assert!(flac.len() < pcm.len() * 2);
    let path = std::env::temp_dir().join(format!("rtk-flac-{}.flac", std::process::id()));
    std::fs::write(&path, flac).unwrap();
    let result = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-f", "s16le", "-"])
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        result.stdout,
        pcm.iter().flat_map(|n| n.to_le_bytes()).collect::<Vec<_>>()
    );
}
