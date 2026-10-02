//! Inspect CMAF initialization and rebase fragmented MP4 decode clocks without copying samples.
use crate::Error;
use std::collections::{HashMap, HashSet};
fn bad() -> Error {
    Error::Playlist("damaged fragmented MP4".into())
}
fn u32at(b: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        b.get(at..at + 4).ok_or_else(bad)?.try_into().unwrap(),
    ))
}
fn decode_time(b: &[u8]) -> Result<(u64, bool), Error> {
    match b.first() {
        Some(0) => Ok((u64::from(u32at(b, 4)?), false)),
        Some(1) => Ok((
            u64::from_be_bytes(b.get(4..12).ok_or_else(bad)?.try_into().unwrap()),
            true,
        )),
        _ => Err(bad()),
    }
}
fn boxes(b: &[u8]) -> Result<Vec<(usize, usize, [u8; 4])>, Error> {
    let mut out = vec![];
    let mut at = 0;
    while at < b.len() {
        if b.len() - at < 8 {
            return Err(bad());
        }
        let n = u32at(b, at)? as usize;
        if n < 8 || n > b.len() - at {
            return Err(bad());
        }
        out.push((at + 8, at + n, b[at + 4..at + 8].try_into().unwrap()));
        at += n;
    }
    Ok(out)
}
fn child<'a>(b: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], Error> {
    boxes(b)?
        .into_iter()
        .find(|(_, _, k)| k == kind)
        .map(|(a, z, _)| &b[a..z])
        .ok_or_else(bad)
}

/// Rebase supported CMAF fragments to a continuous per-track decode timeline.
#[derive(Default, Clone)]
pub struct Rebaser {
    scales: HashMap<u32, u32>,
    ends: HashMap<u32, u64>,
    defaults: HashMap<u32, u32>,
    origin: Option<u64>,
}
impl Rebaser {
    /// Validate an initialization and return its browser MIME type. State changes only on success.
    pub fn init(&mut self, bytes: &[u8]) -> Result<String, Error> {
        let mut next = self.clone();
        let mime = next.init_inner(bytes)?;
        *self = next;
        Ok(mime)
    }
    fn init_inner(&mut self, bytes: &[u8]) -> Result<String, Error> {
        let moov = child(bytes, b"moov")?;
        let mut codecs = vec![];
        let mut tracks = HashSet::new();
        self.defaults.clear();
        for (a, z, kind) in boxes(moov)? {
            if kind == *b"mvex" {
                for (a, z, k) in boxes(&moov[a..z])? {
                    if k == *b"trex" {
                        let p = &child(moov, b"mvex")?[a..z];
                        self.defaults.insert(u32at(p, 4)?, u32at(p, 12)?);
                    }
                }
            }
            if kind != *b"trak" {
                continue;
            }
            let trak = &moov[a..z];
            let tkhd = child(trak, b"tkhd")?;
            let id = u32at(tkhd, if tkhd.first() == Some(&1) { 20 } else { 12 })?;
            if id == 0 || !tracks.insert(id) {
                return Err(bad());
            }
            let mdia = child(trak, b"mdia")?;
            let mdhd = child(mdia, b"mdhd")?;
            let scale = u32at(mdhd, if mdhd.first() == Some(&1) { 20 } else { 12 })?;
            if scale == 0 {
                return Err(bad());
            }
            if let Some(old) = self.scales.insert(id, scale)
                && old != scale
                && let Some(end) = self.ends.get_mut(&id)
            {
                *end = ((*end as u128) * u128::from(scale) / u128::from(old)) as u64;
            }
            let stsd = child(child(child(mdia, b"minf")?, b"stbl")?, b"stsd")?;
            let entry = stsd.get(8..).ok_or_else(bad)?;
            let (_, _, kind) = *boxes(entry)?.first().ok_or_else(bad)?;
            let codec = match &kind {
                b"avc1" | b"avc3" => {
                    let cfg = child(entry.get(86..).ok_or_else(bad)?, b"avcC")?;
                    if cfg.len() < 4 {
                        return Err(bad());
                    }
                    format!("avc1.{:02x}{:02x}{:02x}", cfg[1], cfg[2], cfg[3])
                }
                b"mp4a" => "mp4a.40.2".into(),
                b"fLaC" => "flac".into(),
                b"ac-3" => "ac-3".into(),
                b"ec-3" => "ec-3".into(),
                _ => {
                    return Err(Error::Unsupported(format!(
                        "CMAF codec {} is unsupported",
                        String::from_utf8_lossy(&kind)
                    )));
                }
            };
            codecs.push(codec);
        }
        if codecs.is_empty() {
            return Err(bad());
        }
        if self.defaults.keys().any(|id| !tracks.contains(id)) {
            return Err(bad());
        }
        self.scales.retain(|id, _| tracks.contains(id));
        self.ends.retain(|id, _| tracks.contains(id));
        let video = codecs.iter().any(|c| c.starts_with("avc1"));
        Ok(format!(
            "{}/mp4; codecs=\"{}\"",
            if video { "video" } else { "audio" },
            codecs.join(",")
        ))
    }
    /// Rewrite decode timestamps without copying media samples. Invalid input changes neither
    /// the input bytes nor the running clocks. EXTINF supplies duration only when MP4 does not.
    pub fn fragment(&mut self, bytes: &mut [u8], fallback_seconds: f64) -> Result<(), Error> {
        let mut next = self.clone();
        next.fragment_inner(bytes, fallback_seconds)?;
        *self = next;
        Ok(())
    }
    fn fragment_inner(&mut self, bytes: &mut [u8], fallback_seconds: f64) -> Result<(), Error> {
        let mut patches = vec![];
        if self.origin.is_none() {
            let mut origin = None;
            for (a, z, k) in boxes(bytes)? {
                if k == *b"moof" {
                    for (t, e, k) in boxes(&bytes[a..z])? {
                        if k == *b"traf" {
                            let traf = &bytes[a + t..a + e];
                            let id = u32at(child(traf, b"tfhd")?, 4)?;
                            let scale = *self.scales.get(&id).ok_or_else(bad)?;
                            let p = child(traf, b"tfdt")?;
                            let (base, _) = decode_time(p)?;
                            let time = (u128::from(base) * 1_000_000 / u128::from(scale)) as u64;
                            origin = Some(origin.map_or(time, |v: u64| v.min(time)));
                        }
                    }
                }
            }
            self.origin = origin;
        }
        for (a, z, k) in boxes(bytes)? {
            if k != *b"moof" {
                continue;
            }
            for (t, e, k) in boxes(&bytes[a..z])? {
                if k != *b"traf" {
                    continue;
                }
                let start = a + t;
                let end = a + e;
                let traf = &bytes[start..end];
                let header = child(traf, b"tfhd")?;
                let flags = u32at(header, 0)? & 0xffffff;
                let id = u32at(header, 4)?;
                let scale = *self.scales.get(&id).ok_or_else(bad)?;
                let mut offset = 8;
                if flags & 1 != 0 {
                    offset += 8;
                }
                if flags & 2 != 0 {
                    offset += 4;
                }
                let default = if flags & 8 != 0 {
                    u32at(header, offset)?
                } else {
                    *self.defaults.get(&id).unwrap_or(&0)
                };
                let mut duration = 0u64;
                let mut tfdt = None;
                for (p, q, k) in boxes(traf)? {
                    let b = &traf[p..q];
                    if k == *b"tfdt" {
                        let (_, wide) = decode_time(b)?;
                        if tfdt.replace((start + p, wide)).is_some() {
                            return Err(bad());
                        }
                    }
                    if k == *b"trun" {
                        let flags = u32at(b, 0)? & 0xffffff;
                        let count = u32at(b, 4)?;
                        let mut at = 8;
                        if flags & 1 != 0 {
                            at += 4;
                        }
                        if flags & 4 != 0 {
                            at += 4;
                        }
                        if at > b.len() {
                            return Err(bad());
                        }
                        let width = (flags & 0xf00).count_ones() as usize * 4;
                        if width > 0
                            && usize::try_from(count).unwrap_or(usize::MAX)
                                > b.len().saturating_sub(at) / width
                        {
                            return Err(bad());
                        }
                        if width == 0 {
                            duration = duration
                                .checked_add(u64::from(default) * u64::from(count))
                                .ok_or_else(bad)?;
                            continue;
                        }
                        for _ in 0..count {
                            duration = duration
                                .checked_add(u64::from(if flags & 0x100 != 0 {
                                    u32at(b, at)?
                                } else {
                                    default
                                }))
                                .ok_or_else(bad)?;
                            at += width;
                        }
                    }
                }
                let (at, wide) = tfdt.ok_or_else(bad)?;
                let need = if wide { 12 } else { 8 };
                if bytes.get(at..at + need).is_none() {
                    return Err(bad());
                }
                let original = if wide {
                    u64::from_be_bytes(bytes[at + 4..at + 12].try_into().unwrap())
                } else {
                    u64::from(u32at(bytes, at + 4)?)
                };
                let offset =
                    (u128::from(self.origin.unwrap_or(0)) * u128::from(scale) / 1_000_000) as u64;
                let base = *self
                    .ends
                    .get(&id)
                    .unwrap_or(&original.saturating_sub(offset));
                if wide {
                    patches.push((at + 4, base.to_be_bytes().to_vec()));
                } else {
                    let base = u32::try_from(base).map_err(|_| {
                        Error::Unsupported("32-bit CMAF decode time overflow".into())
                    })?;
                    patches.push((at + 4, base.to_be_bytes().to_vec()));
                }
                if duration == 0 {
                    duration = (fallback_seconds * scale as f64).max(1.0) as u64;
                }
                self.ends
                    .insert(id, base.checked_add(duration).ok_or_else(bad)?);
            }
        }
        if patches.is_empty() {
            return Err(bad());
        }
        for (at, patch) in patches {
            bytes[at..at + patch.len()].copy_from_slice(&patch);
        }
        Ok(())
    }
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend(((payload.len() + 8) as u32).to_be_bytes());
    out.extend(kind);
    out.extend(payload);
    out
}

/// Merge a separate audio initialization into a video initialization, assigning audio track 2.
pub fn merge_init(video: &[u8], audio: &[u8]) -> Result<(Vec<u8>, String), Error> {
    let video_moov = child(video, b"moov")?;
    let audio_moov = child(audio, b"moov")?;
    let mut traks = vec![];
    let mut trexes = vec![];
    for (a, z, k) in boxes(video_moov)? {
        if k == *b"trak" {
            let t = &video_moov[a..z];
            let handler = child(child(t, b"mdia")?, b"hdlr")?;
            if handler.get(8..12) == Some(&b"vide"[..]) {
                let header = child(t, b"tkhd")?;
                if u32at(header, if header.first() == Some(&1) { 20 } else { 12 })? != 1
                    || !traks.is_empty()
                {
                    return Err(Error::Unsupported(
                        "external audio merge requires one video track with ID 1".into(),
                    ));
                }
                traks.extend(bx(b"trak", t));
            }
        }
        if k == *b"mvex" {
            for (a, z, k) in boxes(&video_moov[a..z])? {
                if k == *b"trex" {
                    let p = &child(video_moov, b"mvex")?[a..z];
                    if u32at(p, 4)? == 1 {
                        trexes.extend(bx(b"trex", p));
                    }
                }
            }
        }
    }
    let mut audio_id = None;
    for (a, z, k) in boxes(audio_moov)? {
        if k != *b"trak" {
            continue;
        }
        let t = &audio_moov[a..z];
        let handler = child(child(t, b"mdia")?, b"hdlr")?;
        if handler.get(8..12) != Some(&b"soun"[..]) {
            continue;
        }
        let header = child(t, b"tkhd")?;
        let id = u32at(header, if header.first() == Some(&1) { 20 } else { 12 })?;
        if audio_id.replace(id).is_some() {
            return Err(Error::Unsupported(
                "external audio initialization contains multiple audio tracks".into(),
            ));
        }
        let mut track = t.to_vec();
        for (a, _, k) in boxes(&track)? {
            if k == *b"tkhd" {
                let off = if track.get(a) == Some(&1) { 20 } else { 12 };
                track
                    .get_mut(a + off..a + off + 4)
                    .ok_or_else(bad)?
                    .copy_from_slice(&2u32.to_be_bytes());
            }
        }
        traks.extend(bx(b"trak", &track));
    }
    let audio_id = audio_id.ok_or_else(bad)?;
    let mut trex = None;
    if let Ok(mvex) = child(audio_moov, b"mvex") {
        for (a, z, k) in boxes(mvex)? {
            if k == *b"trex" && u32at(&mvex[a..z], 4)? == audio_id {
                let mut p = mvex[a..z].to_vec();
                p[4..8].copy_from_slice(&2u32.to_be_bytes());
                trex = Some(p);
            }
        }
    }
    let trex = trex.unwrap_or_else(|| {
        let mut p = vec![0; 4];
        for n in [2u32, 1, 0, 0, 0] {
            p.extend(n.to_be_bytes());
        }
        p
    });
    trexes.extend(bx(b"trex", &trex));
    let mut mvhd = child(video_moov, b"mvhd")?.to_vec();
    let n = mvhd.len();
    if n < 4 {
        return Err(bad());
    }
    mvhd[n - 4..].copy_from_slice(&3u32.to_be_bytes());
    let mut moov = bx(b"mvhd", &mvhd);
    moov.extend(traks);
    moov.extend(bx(b"mvex", &trexes));
    let mut bytes = bx(b"ftyp", child(video, b"ftyp")?);
    bytes.extend(bx(b"moov", &moov));
    let mime = Rebaser::default().init(&bytes)?;
    Ok((bytes, mime))
}

/// Remap a separate audio fragment to track 2 and shift its clock in audio timescale ticks.
/// Invalid input leaves the bytes unchanged.
pub fn audio_fragment(bytes: &mut [u8], offset: i64) -> Result<(), Error> {
    let mut patches = vec![];
    for (a, z, k) in boxes(bytes)? {
        if k == *b"moof" {
            for (t, e, k) in boxes(&bytes[a..z])? {
                if k == *b"traf" {
                    let start = a + t;
                    for (p, q, k) in boxes(&bytes[start..a + e])? {
                        let at = start + p;
                        if k == *b"tfhd" {
                            bytes
                                .get(at + 4..at + 8)
                                .filter(|_| q - p >= 8)
                                .ok_or_else(bad)?;
                            patches.push((at + 4, 2u32.to_be_bytes().to_vec()));
                        }
                        if k == *b"tfdt" {
                            let (base, wide) = decode_time(&bytes[at..start + q])?;
                            let moved = i128::from(base) + i128::from(offset);
                            let moved = u64::try_from(moved.max(0)).map_err(|_| bad())?;
                            if wide {
                                patches.push((at + 4, moved.to_be_bytes().to_vec()));
                            } else {
                                patches.push((
                                    at + 4,
                                    u32::try_from(moved)
                                        .map_err(|_| bad())?
                                        .to_be_bytes()
                                        .to_vec(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    if patches.is_empty() {
        return Err(bad());
    }
    for (at, patch) in patches {
        bytes[at..at + patch.len()].copy_from_slice(&patch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damaged_fragments_do_not_partially_change_bytes_or_clocks() {
        let out = crate::Transmuxer::default()
            .push(include_bytes!("../tests/fixtures/bbb_480p.ts"))
            .unwrap();
        let mut r = Rebaser::default();
        r.init(&out.init.unwrap().bytes).unwrap();
        let moof = &out.fragments[0].moof;
        let mut bad_traf = bx(b"tfhd", &[0, 0, 0, 0, 0, 0, 0, 1]);
        bad_traf.extend(bx(b"tfdt", &[]));
        let mut bytes = moof.clone();
        bytes.extend(bx(b"moof", &bx(b"traf", &bad_traf)));
        let original = bytes.clone();
        assert!(r.fragment(&mut bytes, 2.0).is_err());
        assert_eq!(bytes, original);
        assert!(r.ends.is_empty());
        assert!(r.origin.is_none());
        assert!(audio_fragment(&mut bytes, 0).is_err());
        assert_eq!(bytes, original);

        // Exercise every truncation boundary, including full boxes followed by partial boxes.
        for end in 0..moof.len() {
            let mut bytes = moof[..end].to_vec();
            let original = bytes.clone();
            let mut attempt = r.clone();
            if attempt.fragment(&mut bytes, 2.0).is_err() {
                assert_eq!(bytes, original);
                assert_eq!(attempt.ends, r.ends);
                assert_eq!(attempt.origin, r.origin);
            }
            if audio_fragment(&mut bytes, 0).is_err() {
                assert_eq!(bytes, original);
            }
        }
        let mut valid = moof.clone();
        r.fragment(&mut valid, 2.0).unwrap();
        assert!(r.ends.values().all(|end| *end > 0));
    }

    #[test]
    fn separate_audio_tracks_merge_into_one_initialization() {
        let clip = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let mut video = crate::ts::demux(clip).unwrap();
        video.audio.clear();
        video.aac = None;
        video.video_data.splice(0..0, [0; 8]);
        let mut audio = crate::ts::demux(clip).unwrap();
        audio.video.clear();
        audio.video_data.clear();
        audio.sps = None;
        audio.pps = None;
        let v = crate::Transmuxer::default().assemble(video).unwrap();
        let a = crate::Transmuxer::default().assemble(audio).unwrap();
        let (init, mime) = merge_init(&v.init.unwrap().bytes, &a.init.unwrap().bytes).unwrap();
        assert!(mime.contains("avc1") && mime.contains("mp4a"));
        let mut r = Rebaser::default();
        r.init(&init).unwrap();
        assert_eq!(r.scales.len(), 2);
        let mut f = a.fragments[0].moof.clone();
        audio_fragment(&mut f, 0).unwrap();
        assert!(f.windows(4).any(|w| w == b"tfhd"));
    }

    #[test]
    fn generated_fragments_rebase_without_changing_samples() {
        let mut t = crate::Transmuxer::default();
        let out = t
            .push(include_bytes!("../tests/fixtures/bbb_480p.ts"))
            .unwrap();
        let mut r = Rebaser::default();
        assert!(r.init(&out.init.unwrap().bytes).unwrap().contains("avc1"));
        for _ in 0..3 {
            for f in &out.fragments {
                let mut bytes = [f.moof.as_slice(), f.mdat.as_slice()].concat();
                let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap();
                let payload = bytes[mdat..].to_vec();
                r.fragment(&mut bytes, 2.0).unwrap();
                assert_eq!(&bytes[mdat..], payload);
            }
        }
        assert!(r.ends.values().all(|v| *v > 0));
        assert!(r.fragment(&mut [0; 16], 2.0).is_err());
    }
}
