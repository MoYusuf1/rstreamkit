//! Just enough of an H.264 SPS parser to read the picture size. It stops right after the
//! cropping fields, so oddities in the trailing VUI data can never make it fail (a general-purpose
//! parser rejected a valid 1080p stream from the test set for exactly that reason).

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(bit as u32)
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0, |acc, _| Some(acc << 1 | self.bit()?))
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some(((1u64 << zeros) - 1 + self.bits(zeros)? as u64) as u32)
    }

    /// Signed Exp-Golomb.
    fn se(&mut self) -> Option<i32> {
        let k = self.ue()?;
        Some(if k % 2 == 1 {
            (k / 2 + 1) as i32
        } else {
            -((k / 2) as i32)
        })
    }
}

/// Drops the 0x03 emulation-prevention bytes that keep start codes out of NAL payloads.
fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Width and height in pixels from an SPS NAL unit (header byte included, no start code).
pub fn dimensions(sps_nal: &[u8]) -> Option<(u32, u32)> {
    let rbsp = unescape(sps_nal.get(1..)?);
    read_size(&mut Bits {
        data: &rbsp,
        pos: 0,
    })
    .map(|(w, h, _)| (w, h))
}

/// Whether the pictures are interlaced (two fields per frame). Browsers show those combed and at
/// half the motion rate, so they are worth converting. `None` if the SPS can't be read.
pub fn interlaced(sps_nal: &[u8]) -> Option<bool> {
    let rbsp = unescape(sps_nal.get(1..)?);
    read_size(&mut Bits {
        data: &rbsp,
        pos: 0,
    })
    .map(|(_, _, interlaced)| interlaced)
}

/// Shape of one pixel (width, height) from the SPS's VUI data. Broadcast SD is anamorphic: a
/// 720x576 picture is 16:9 on screen only if the player knows the pixels are 64:45, and without
/// this the picture shows stretched. `None` means "not stated", which players treat as square.
pub fn pixel_aspect(sps_nal: &[u8]) -> Option<(u32, u32)> {
    let rbsp = unescape(sps_nal.get(1..)?);
    let mut r = Bits {
        data: &rbsp,
        pos: 0,
    };
    read_size(&mut r)?;
    if r.bit()? == 0 || r.bit()? == 0 {
        return None; // no VUI, or no aspect ratio in it
    }
    let (w, h) = match r.bits(8)? {
        1 => (1, 1),
        2 => (12, 11),
        3 => (10, 11),
        4 => (16, 11),
        5 => (40, 33),
        6 => (24, 11),
        7 => (20, 11),
        8 => (32, 11),
        9 => (80, 33),
        10 => (18, 11),
        11 => (15, 11),
        12 => (64, 33),
        13 => (160, 99),
        14 => (4, 3),
        15 => (3, 2),
        16 => (2, 1),
        255 => (r.bits(16)?, r.bits(16)?),
        _ => return None,
    };
    (w > 0 && h > 0).then_some((w, h))
}

/// Reads the SPS up to and including the cropping fields, leaving `r` at the VUI flag.
fn read_size(r: &mut Bits) -> Option<(u32, u32, bool)> {
    let profile = r.bits(8)?;
    r.bits(16)?; // constraint flags + level
    r.ue()?; // SPS id

    let mut chroma = 1; // 4:2:0 unless the profile says otherwise
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma = r.ue()?;
        if chroma == 3 {
            r.bit()?; // separate colour planes
        }
        r.ue()?; // luma bit depth
        r.ue()?; // chroma bit depth
        r.bit()?; // lossless bypass
        if r.bit()? == 1 {
            // Scaling lists: their values don't matter, but they have to be read past.
            for i in 0..(if chroma != 3 { 8 } else { 12 }) {
                if r.bit()? == 1 {
                    let (mut last, mut next) = (8, 8);
                    for _ in 0..(if i < 6 { 16 } else { 64 }) {
                        if next != 0 {
                            next = (last + r.se()? + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }

    r.ue()?; // log2_max_frame_num
    match r.ue()? {
        0 => drop(r.ue()?), // log2_max_pic_order_cnt_lsb
        1 => {
            r.bit()?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()? {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue()?; // max reference frames
    r.bit()?; // gaps in frame numbers allowed

    let (mbs_wide, map_units_high) = (r.ue()? as u64 + 1, r.ue()? as u64 + 1);
    let frame_mbs_only = r.bit()? as u64;
    if frame_mbs_only == 0 {
        r.bit()?; // mb-adaptive frame/field
    }
    r.bit()?; // direct 8x8 inference

    let (mut left, mut right, mut top, mut bottom) = (0u64, 0u64, 0u64, 0u64);
    if r.bit()? == 1 {
        (left, right, top, bottom) = (
            r.ue()? as u64,
            r.ue()? as u64,
            r.ue()? as u64,
            r.ue()? as u64,
        );
    }
    let field = 2 - frame_mbs_only; // interlaced pictures are two fields high
    let (unit_x, unit_y) = match chroma {
        1 => (2, 2 * field),
        2 => (2, field),
        _ => (1, field),
    };
    let width = (mbs_wide * 16).checked_sub(unit_x * (left + right))?;
    let height = (field * map_units_high * 16).checked_sub(unit_y * (top + bottom))?;
    (width > 0 && height > 0).then_some((
        u32::try_from(width).ok()?,
        u32::try_from(height).ok()?,
        frame_mbs_only == 0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Real SPS units from four renditions of one stream; the sizes are the RESOLUTION values the
    /// stream's own master playlist declares. 240p needs bottom cropping (192 -> 184), 1080p too (1088 -> 1080).
    #[test]
    fn reads_sizes_from_real_streams() {
        for (name, sps, size) in [
            (
                "240p baseline",
                "6742c00dd981419f970110000003001000000303c0f142a680",
                (320, 184),
            ),
            (
                "380p baseline",
                "6742c016d9008025b0110000030001000003003c0f162e48",
                (512, 288),
            ),
            (
                "480p high",
                "6764001facd940d43db011000003000100000300780f183196",
                (848, 480),
            ),
            (
                "1080p high",
                "67640028acd980780227e5c044000003000400000301e03c60c668",
                (1920, 1080),
            ),
        ] {
            assert_eq!(dimensions(&hex(sps)), Some(size), "{name}");
        }
    }

    /// The 480p test stream says 1:1 outright; the anamorphic PAL fixture (`setsar=64/45`, ffprobe
    /// agrees) says 64:45; a stream with no VUI says nothing.
    /// Progressive streams say so in their SPS; an interlaced one (made by ffmpeg with
    /// `ildct+ilme`, and confirmed by ffprobe as `tt`) says it isn't.
    #[test]
    fn tells_interlaced_from_progressive() {
        for sps in [
            "6742c00dd981419f970110000003001000000303c0f142a680",
            "6764001facd940d43db011000003000100000300780f183196",
            "67640028acd980780227e5c044000003000400000301e03c60c668",
        ] {
            assert_eq!(interlaced(&hex(sps)), Some(false));
        }
        let field =
            crate::ts::demux(include_bytes!("../tests/fixtures/interlaced_576i.ts")).unwrap();
        assert_eq!(interlaced(&field.sps.unwrap()), Some(true));
        assert_eq!(interlaced(&[0x67]), None);
    }

    #[test]
    fn reads_the_pixel_aspect() {
        assert_eq!(
            pixel_aspect(&hex("6764001facd940d43db011000003000100000300780f183196")),
            Some((1, 1))
        );
        let pal = crate::ts::demux(include_bytes!("../tests/fixtures/pal_anamorphic.ts")).unwrap();
        assert_eq!(pixel_aspect(&pal.sps.unwrap()), Some((64, 45)));
    }

    #[test]
    fn garbage_and_truncation_are_none_not_panics() {
        assert_eq!(dimensions(&[]), None);
        assert_eq!(dimensions(&[0x67]), None);
        assert_eq!(pixel_aspect(&[0x67, 0, 0]), None);
        assert_eq!(
            dimensions(&hex("6764001facd940d4")),
            None,
            "cut off before the picture size"
        );
        assert_eq!(
            dimensions(&[0x67, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            None,
            "all zeros never terminates an Exp-Golomb code"
        );
    }

    #[test]
    fn exp_golomb_and_unescape() {
        // 1 -> 0, 010 -> 1, 011 -> 2, 00100 -> 3 ; se: 010 -> +1, 011 -> -1
        let mut r = Bits {
            data: &[0b1010_0110, 0b0100_0000],
            pos: 0,
        };
        assert_eq!(
            (r.ue(), r.ue(), r.ue(), r.ue()),
            (Some(0), Some(1), Some(2), Some(3))
        );
        let mut r = Bits {
            data: &[0b0100_1100],
            pos: 0,
        };
        assert_eq!((r.se(), r.se()), (Some(1), Some(-1)));
        assert_eq!(unescape(&[1, 0, 0, 3, 1, 0, 0, 3]), vec![1, 0, 0, 1, 0, 0]);
    }
}
