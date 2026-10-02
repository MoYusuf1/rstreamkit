//! HLS playlist parsing: the handful of tags a player needs, nothing more.
//!
//! ponytail: no ABR (one variant is picked once), no AES-128 or fMP4 segments, and demuxed
//! audio renditions (EXT-X-MEDIA) are ignored, so those streams play without sound.

use crate::Error;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub codecs: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub audio_group: Option<String>,
    pub renditions: Vec<Rendition>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rendition {
    pub group: String,
    pub name: String,
    pub language: Option<String>,
    pub uri: Option<String>,
    pub default: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub uri: String,
    pub iv: Option<[u8; 16]>,
}
impl Key {
    pub fn iv_for(&self, sequence: u64) -> [u8; 16] {
        self.iv.unwrap_or_else(|| {
            let mut iv = [0; 16];
            iv[8..].copy_from_slice(&sequence.to_be_bytes());
            iv
        })
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    pub uri: String,
    pub byte_range: Option<std::ops::Range<u64>>,
}

fn byte_range(value: &str, offset: Option<u64>) -> Result<std::ops::Range<u64>, Error> {
    let (len, at) = value
        .split_once('@')
        .map_or((value, None), |(l, a)| (l, Some(a)));
    let len: u64 = len
        .parse()
        .map_err(|_| Error::Playlist("invalid byte range".into()))?;
    let at = match at {
        Some(a) => a
            .parse()
            .map_err(|_| Error::Playlist("invalid range offset".into()))?,
        None => offset
            .ok_or_else(|| Error::Playlist("implicit byte range has no previous range".into()))?,
    };
    let end = at
        .checked_add(len)
        .filter(|_| len > 0)
        .ok_or_else(|| Error::Playlist("invalid byte range length".into()))?;
    Ok(at..end)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub uri: String,
    /// EXT-X-MEDIA-SEQUENCE plus position: stable across live playlist refreshes.
    pub seq: u64,
    /// EXTINF, in seconds.
    pub duration: f64,
    pub discontinuity: bool,
    pub discontinuity_sequence: u64,
    /// RFC3339 source timestamp, retained without timezone conversion.
    pub program_date_time: Option<String>,
    pub byte_range: Option<std::ops::Range<u64>>,
    pub key: Option<Key>,
    pub map: Option<Map>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub segments: Vec<Segment>,
    pub target_duration: f64,
    /// EXT-X-ENDLIST: a finished (VOD) playlist that will not grow.
    pub ended: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Parsed {
    Master(Vec<Variant>),
    Media(Media),
}

impl Media {
    /// Duration of the currently advertised playback/DVR window.
    pub fn duration(&self) -> f64 {
        self.segments.iter().map(|s| s.duration).sum()
    }
}

impl Variant {
    /// Whether the declared codecs fit the Rust H.264/AAC/decoded-sound pipeline.
    /// Missing CODECS remains probeable; an explicit unknown codec does not.
    pub fn supported(&self) -> bool {
        self.codecs.as_ref().is_none_or(|c| {
            c.split(',').all(|c| {
                let c = c.trim();
                c.starts_with("avc1.")
                    || c.starts_with("avc3.")
                    || c.starts_with("mp4a.40.")
                    || matches!(c, "ac-3" | "ec-3")
            })
        })
    }
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
    let (mut renditions, mut key, mut map, mut pending_range) = (vec![], None, None, None);
    let mut previous_range: Option<(String, u64)> = None;
    let (mut target_duration, mut first_seq, mut ended) = (0.0, 0u64, false);
    let (mut duration, mut discontinuity, mut discontinuity_sequence, mut date) =
        (0.0, false, 0u64, None);
    let mut pending_variant = None; // bandwidth from the STREAM-INF line whose URI comes next
    for line in lines {
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_variant = Some(Variant {
                bandwidth: attr(rest, "BANDWIDTH")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                codecs: attr(rest, "CODECS").map(str::to_owned),
                resolution: attr(rest, "RESOLUTION")
                    .and_then(|v| v.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))),
                audio_group: attr(rest, "AUDIO").map(str::to_owned),
                ..Variant::default()
            });
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            duration = rest
                .split(',')
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .ok_or_else(|| Error::Playlist("invalid EXTINF duration".into()))?;
        } else if let Some(rest) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
            discontinuity_sequence = rest
                .parse()
                .map_err(|_| Error::Playlist("invalid discontinuity sequence".into()))?;
        } else if line == "#EXT-X-DISCONTINUITY" {
            discontinuity = true;
            discontinuity_sequence = discontinuity_sequence
                .checked_add(1)
                .ok_or_else(|| Error::Playlist("discontinuity sequence overflow".into()))?;
        } else if let Some(rest) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            date = Some(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            target_duration = rest.trim().parse().unwrap_or(0.0);
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            first_seq = rest.trim().parse().unwrap_or(0);
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
            if attr(rest, "TYPE") == Some("AUDIO") {
                renditions.push(Rendition {
                    group: attr(rest, "GROUP-ID").unwrap_or("").into(),
                    name: attr(rest, "NAME").unwrap_or("").into(),
                    language: attr(rest, "LANGUAGE").map(str::to_owned),
                    uri: attr(rest, "URI").map(str::to_owned),
                    default: attr(rest, "DEFAULT") == Some("YES"),
                });
            }
        } else if let Some(rest) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_range = Some(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("#EXT-X-KEY:") {
            match attr(rest, "METHOD") {
                Some("NONE") => key = None,
                Some("AES-128") if attr(rest, "KEYFORMAT").is_none_or(|k| k == "identity") => {
                    let uri = attr(rest, "URI")
                        .ok_or_else(|| Error::Playlist("AES-128 key has no URI".into()))?;
                    let iv = attr(rest, "IV")
                        .map(|v| {
                            let hex = v
                                .strip_prefix("0x")
                                .or_else(|| v.strip_prefix("0X"))
                                .ok_or_else(|| Error::Playlist("invalid key IV".into()))?;
                            if hex.len() > 32
                                || hex.is_empty()
                                || !hex.bytes().all(|b| b.is_ascii_hexdigit())
                            {
                                return Err(Error::Playlist("invalid key IV".into()));
                            }
                            let hex = format!("{hex:0>32}");
                            let mut iv = [0; 16];
                            for (i, b) in iv.iter_mut().enumerate() {
                                *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                                    .map_err(|_| Error::Playlist("invalid key IV".into()))?;
                            }
                            Ok(iv)
                        })
                        .transpose()?;
                    key = Some(Key {
                        uri: uri.into(),
                        iv,
                    });
                }
                _ => {
                    return Err(Error::Unsupported(
                        "HLS encryption method or key format is unsupported".into(),
                    ));
                }
            }
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
            map = Some(Map {
                uri: attr(rest, "URI")
                    .ok_or_else(|| Error::Playlist("map has no URI".into()))?
                    .into(),
                byte_range: attr(rest, "BYTERANGE")
                    .map(|v| byte_range(v, None))
                    .transpose()?,
            });
        } else if line.starts_with('#') {
            // Other tags (EXTINF durations, version, program date...) and comments don't matter here.
        } else if let Some(mut variant) = pending_variant.take() {
            variant.uri = line.into();
            variants.push(variant);
        } else {
            segments.push(Segment {
                uri: line.into(),
                seq: first_seq
                    .checked_add(segments.len() as u64)
                    .ok_or_else(|| Error::Playlist("media sequence overflow".into()))?,
                duration,
                discontinuity,
                discontinuity_sequence,
                program_date_time: date.take(),
                byte_range: pending_range
                    .take()
                    .map(|v| {
                        byte_range(
                            &v,
                            previous_range
                                .as_ref()
                                .filter(|(uri, _)| uri == line)
                                .map(|(_, end)| *end),
                        )
                    })
                    .transpose()?,
                key: key.clone(),
                map: map.clone(),
            });
            previous_range = segments
                .last()
                .and_then(|s| s.byte_range.as_ref().map(|r| (s.uri.clone(), r.end)));
            duration = 0.0;
            discontinuity = false;
        }
    }

    for v in &mut variants {
        v.renditions = renditions
            .iter()
            .filter(|r| Some(&r.group) == v.audio_group.as_ref())
            .cloned()
            .collect();
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
    pick_variant_for(variants, 4_000_000)
}

/// Select a codec-compatible variant within a measured bandwidth budget.
pub fn pick_variant_for(variants: &[Variant], budget: u64) -> Option<&Variant> {
    variants
        .iter()
        .filter(|v| v.supported() && v.bandwidth <= budget)
        .max_by_key(|v| v.bandwidth)
        .or_else(|| {
            variants
                .iter()
                .filter(|v| v.supported())
                .min_by_key(|v| v.bandwidth)
        })
}

pub const RAW_STREAM: &str = "this is a raw MPEG-TS stream, not an HLS playlist";

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
                ..Variant::default()
            },
            Variant {
                uri: "b".into(),
                bandwidth: 8_000_000,
                ..Variant::default()
            },
        ];
        assert_eq!(pick_variant(&only_big).unwrap().uri, "b"); // everything over budget: take the lowest
    }

    #[test]
    fn timing_and_codec_metadata() {
        let Parsed::Media(m) = parse(b"#EXTM3U\n#EXT-X-DISCONTINUITY-SEQUENCE:7\n#EXTINF:2.5,\na.ts\n#EXT-X-DISCONTINUITY\n#EXT-X-PROGRAM-DATE-TIME:2026-10-01T12:00:00Z\n#EXTINF:3.25,\nb.ts\n").unwrap() else { panic!() };
        assert_eq!(m.duration(), 5.75);
        assert_eq!(m.segments[1].discontinuity_sequence, 8);
        assert!(m.segments[1].discontinuity);
        assert_eq!(
            m.segments[1].program_date_time.as_deref(),
            Some("2026-10-01T12:00:00Z")
        );
        let Parsed::Master(v) = parse(b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100,CODECS=\"hvc1.1\"\nhevc\n#EXT-X-STREAM-INF:BANDWIDTH=200,CODECS=\"avc1.64001f,mp4a.40.2\",RESOLUTION=1280x720\navc\n").unwrap() else { panic!() };
        assert_eq!(pick_variant_for(&v, 1000).unwrap().uri, "avc");
        assert_eq!(v[1].resolution, Some((1280, 720)));
        assert!(parse(b"#EXTM3U\n#EXTINF:NaN,\na.ts").is_err());
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
        assert!(matches!(parse(aes.as_bytes()), Ok(Parsed::Media(_))));
        let none = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:6.0,\ns.ts\n";
        assert!(
            parse(none.as_bytes()).is_ok(),
            "METHOD=NONE means unencrypted"
        );
        let fmp4 =
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\ns.m4s\n";
        assert!(matches!(parse(fmp4.as_bytes()), Ok(Parsed::Media(_))));
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
