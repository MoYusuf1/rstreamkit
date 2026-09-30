//! HLS playlist parsing: the handful of tags a player needs, nothing more.
//!
//! ponytail: no ABR (one variant is picked once), no AES-128 or fMP4 segments, and demuxed
//! audio renditions (EXT-X-MEDIA) are ignored, so those streams play without sound.

use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub uri: String,
    /// EXT-X-MEDIA-SEQUENCE plus position: stable across live playlist refreshes.
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub segments: Vec<Segment>,
    pub target_duration: f64,
    /// EXT-X-ENDLIST: a finished (VOD) playlist that will not grow.
    pub ended: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Master(Vec<Variant>),
    Media(Media),
}

/// Value of `key` in an attribute list like `A=1,B="x,y"`, quotes stripped.
/// Commas inside quotes don't split (CODECS="mp4a.40.2,avc1.64001f" is one value).
fn attr<'a>(list: &'a str, key: &str) -> Option<&'a str> {
    let (mut start, mut quoted) = (0, false);
    let mut parts = vec![];
    for (i, c) in list.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&list[start..]);
    parts.into_iter().find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"'))
    })
}

pub fn parse(bytes: &[u8]) -> Result<Parsed, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Playlist("not text".into()))?;
    let mut lines = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    if lines.next() != Some("#EXTM3U") {
        return Err(Error::Playlist(
            "missing #EXTM3U header (not an HLS playlist)".into(),
        ));
    }

    let (mut variants, mut segments) = (vec![], vec![]);
    let (mut target_duration, mut first_seq, mut ended) = (0.0, 0u64, false);
    let mut pending_variant = None; // bandwidth from the STREAM-INF line whose URI comes next
    for line in lines {
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_variant = Some(
                attr(rest, "BANDWIDTH")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            );
        } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            target_duration = rest.trim().parse().unwrap_or(0.0);
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            first_seq = rest.trim().parse().unwrap_or(0);
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if let Some(rest) = line.strip_prefix("#EXT-X-KEY:") {
            if attr(rest, "METHOD").is_some_and(|m| m != "NONE") {
                return Err(Error::Unsupported(
                    "encrypted HLS (AES-128) is not supported".into(),
                ));
            }
        } else if line.starts_with("#EXT-X-MAP:") {
            return Err(Error::Unsupported(
                "fMP4 HLS segments are not supported yet (only MPEG-TS)".into(),
            ));
        } else if line.starts_with('#') {
            // Other tags (EXTINF durations, version, program date...) and comments don't matter here.
        } else if let Some(bandwidth) = pending_variant.take() {
            variants.push(Variant {
                uri: line.into(),
                bandwidth,
            });
        } else {
            segments.push(Segment {
                uri: line.into(),
                seq: first_seq + segments.len() as u64,
            });
        }
    }

    Ok(if variants.is_empty() {
        Parsed::Media(Media {
            segments,
            target_duration,
            ended,
        })
    } else {
        Parsed::Master(variants)
    })
}

/// Highest bandwidth that still fits a modest budget, else the lowest available.
/// ponytail: the budget is fixed; real ABR would watch download speed.
pub fn pick_variant(variants: &[Variant]) -> Option<&Variant> {
    const BUDGET: u64 = 4_000_000;
    variants
        .iter()
        .filter(|v| v.bandwidth <= BUDGET)
        .max_by_key(|v| v.bandwidth)
        .or_else(|| variants.iter().min_by_key(|v| v.bandwidth))
}

pub const RAW_STREAM: &str =
    "this channel sends a raw MPEG-TS stream instead of an HLS playlist, which isn't supported yet";

/// MPEG-TS packets are 188 bytes and each starts with 0x47.
pub fn looks_like_ts(body: &[u8]) -> bool {
    body.first() == Some(&0x47) && body.get(188) == Some(&0x47)
}

/// What a response that should have been a playlist really was, in words: usually a raw stream,
/// or an error page or short "offline" note from the provider.
pub fn describe_non_playlist(body: &[u8], content_type: &str) -> String {
    if looks_like_ts(body) {
        return RAW_STREAM.into();
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return "the server sent an empty response (the channel may be offline)".into();
    }
    let preview = body
        .iter()
        .take(120)
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let kind = if content_type.is_empty() {
        "something else"
    } else {
        content_type
    };
    format!("the server answered with {kind} instead of a playlist: \"{preview}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n\
        #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=2149280,CODECS=\"mp4a.40.2,avc1.64001f\",RESOLUTION=1280x720\nhd.m3u8\n\
        #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=246440,RESOLUTION=320x184\nld.m3u8\n\
        #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=50000,URI=\"iframes.m3u8\"\n\
        #EXT-X-STREAM-INF:PROGRAM-ID=1,AVERAGE-BANDWIDTH=9,BANDWIDTH=6221600,RESOLUTION=1920x1080\nfhd.m3u8\n";

    #[test]
    fn master_picks_best_variant_within_budget() {
        let Parsed::Master(v) = parse(MASTER.as_bytes()).unwrap() else {
            panic!("expected master")
        };
        assert_eq!(v.len(), 3, "the I-frame-only entry is not a variant");
        assert_eq!(
            v[2].bandwidth, 6_221_600,
            "AVERAGE-BANDWIDTH must not be mistaken for BANDWIDTH"
        );
        assert_eq!(pick_variant(&v).unwrap().uri, "hd.m3u8"); // 1080p is over budget
        let only_big = vec![
            Variant {
                uri: "a".into(),
                bandwidth: 9_000_000,
            },
            Variant {
                uri: "b".into(),
                bandwidth: 8_000_000,
            },
        ];
        assert_eq!(pick_variant(&only_big).unwrap().uri, "b"); // everything over budget: take the lowest
    }

    #[test]
    fn attributes_respect_quotes() {
        let list = "PROGRAM-ID=1,CODECS=\"mp4a.40.2,avc1.64001f\",BANDWIDTH=99";
        assert_eq!(attr(list, "CODECS"), Some("mp4a.40.2,avc1.64001f"));
        assert_eq!(attr(list, "BANDWIDTH"), Some("99"));
        assert_eq!(attr(list, "RESOLUTION"), None);
    }

    #[test]
    fn live_media_playlist_keeps_sequence_numbers() {
        let live = "\u{feff}#EXTM3U\r\n#EXT-X-VERSION:3\r\n#EXT-X-TARGETDURATION:6\r\n#EXT-X-MEDIA-SEQUENCE:100\r\n\
            #EXTINF:6.0,\r\nseg100.ts\r\n#EXTINF:6.0,\r\nseg101.ts\r\n";
        let Parsed::Media(m) = parse(live.as_bytes()).unwrap() else {
            panic!("expected media")
        };
        assert!(!m.ended);
        assert_eq!(m.target_duration, 6.0);
        assert_eq!(
            m.segments.iter().map(|s| s.seq).collect::<Vec<_>>(),
            [100, 101]
        );
        assert_eq!(m.segments[1].uri, "seg101.ts");

        let vod = format!("{live}#EXT-X-ENDLIST\n");
        let Parsed::Media(m) = parse(vod.as_bytes()).unwrap() else {
            panic!()
        };
        assert!(m.ended);
    }

    #[test]
    fn empty_live_playlist_is_fine() {
        let Parsed::Media(m) = parse(b"#EXTM3U\n#EXT-X-TARGETDURATION:4\n").unwrap() else {
            panic!()
        };
        assert!(m.segments.is_empty() && !m.ended);
    }

    #[test]
    fn unsupported_features_are_reported_not_misplayed() {
        let aes = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:6.0,\ns.ts\n";
        assert!(matches!(parse(aes.as_bytes()), Err(Error::Unsupported(_))));
        let none = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:6.0,\ns.ts\n";
        assert!(
            parse(none.as_bytes()).is_ok(),
            "METHOD=NONE means unencrypted"
        );
        let fmp4 =
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\ns.m4s\n";
        assert!(matches!(parse(fmp4.as_bytes()), Err(Error::Unsupported(_))));
        assert!(matches!(
            parse(b"<html>not a playlist</html>"),
            Err(Error::Playlist(_))
        ));
        assert!(matches!(
            parse(&[0xff, 0xfe, 0x00]),
            Err(Error::Playlist(_))
        ));
    }

    #[test]
    fn a_response_that_is_not_a_playlist_says_what_it_was() {
        let mut ts = vec![0u8; 400];
        ts[0] = 0x47;
        ts[188] = 0x47;
        assert_eq!(describe_non_playlist(&ts, "video/mp2t"), RAW_STREAM);
        assert!(describe_non_playlist(b"", "text/plain").contains("empty"));
        assert!(describe_non_playlist(b" \r\n", "").contains("empty"));
        let html = describe_non_playlist(
            b"<html>\n  <body>Stream   offline</body></html>",
            "text/html",
        );
        assert_eq!(
            html,
            "the server answered with text/html instead of a playlist: \"<html> <body>Stream offline</body></html>\""
        );
        // Binary noise can't garble the message, and long bodies are cut short.
        let noisy = describe_non_playlist(&[0xff, 0x00, b'o', b'k', 0x1b], "");
        assert!(
            noisy.ends_with("\"ok\"") && noisy.contains("something else"),
            "{noisy}"
        );
        assert!(describe_non_playlist(&[b'x'; 500], "text/plain").len() < 200);
    }
}
