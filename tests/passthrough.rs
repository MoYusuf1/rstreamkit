use rstreamkit::Transmuxer;
use std::{
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn box_bytes<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let at = bytes
        .windows(4)
        .position(|w| w == kind)
        .expect("codec configuration box");
    let n = u32::from_be_bytes(bytes[at - 4..at].try_into().unwrap()) as usize;
    &bytes[at + 4..at - 4 + n]
}
#[test]
fn ac3_configuration_matches_specification_and_ffmpeg() {
    if !Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return;
    }
    for name in ["h264_ac3_st.ts", "h264_ac3_51.ts", "h264_eac3_51.ts"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let bytes = std::fs::read(&path).unwrap();
        let out = Transmuxer::default()
            .passthrough_ac3(true)
            .push(&bytes)
            .unwrap();
        let init = out.init.as_ref().unwrap();
        let enhanced = name.contains("eac3");
        let config = if enhanced { b"dec3" } else { b"dac3" };
        assert!(init.mime.contains(if enhanced { "ec-3" } else { "ac-3" }));
        assert!(!init.mime.contains("flac"));
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let reference =
            std::env::temp_dir().join(format!("rtk-dolby-ref-{}-{id}.mp4", std::process::id()));
        let ours =
            std::env::temp_dir().join(format!("rtk-dolby-ours-{}-{id}.mp4", std::process::id()));
        let mux = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&path)
            .args(["-c", "copy"])
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            mux.status.success(),
            "{}",
            String::from_utf8_lossy(&mux.stderr)
        );
        let reference_bytes = std::fs::read(&reference).unwrap();
        let actual = box_bytes(&init.bytes, config);
        let expected = box_bytes(&reference_bytes, config);
        if enhanced {
            // ETSI TS 102 366 Annex F has five bytes for this single independent stream.
            // Some muxers append the optional zero Atmos-extension flag defined by Dolby:
            // https://ott.dolby.com/OnDelKits/DDP/Dolby_Digital_Plus_Online_Delivery_Kit_v1.5/Documentation/Playback/SDM/help_files/topics/c_id_ddp_atmos_isobmff_2.html
            assert_eq!(actual, [4, 0, 32, 15, 0], "fixture's core dec3 fields");
            assert!(matches!(expected.len(), 5 | 6), "{name}: {expected:?}");
            assert_eq!(actual, &expected[..5], "{name}");
            assert!(
                expected.get(5).is_none_or(|flag| *flag == 0),
                "unsupported extension: {expected:?}"
            );
        } else {
            assert_eq!(actual, expected, "{name}");
        }
        let mut output = init.bytes.clone();
        output.extend(out.fragment());
        std::fs::write(&ours, output).unwrap();
        let decode = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&ours)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(decode.status.success());
        assert!(
            decode.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&decode.stderr)
        );
        std::fs::remove_file(reference).unwrap();
        std::fs::remove_file(ours).unwrap();
    }
}
