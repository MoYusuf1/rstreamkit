//! `rstreamkit INPUT OUTPUT`: repackage a stream or a movie file as fragmented MP4, natively.
//!
//! INPUT is MPEG-TS (one segment or a whole recording), an HLS playlist of local TS or fMP4 segments, an MP4
//! or a Matroska file, or `-` for MPEG-TS on standard input. OUTPUT is a file, or `-` for standard
//! output. Pictures are copied as they are; sound a browser can't play (AC-3, E-AC-3, MP2) becomes
//! FLAC. Nothing here launches or links FFmpeg.
//!
//! Reading, converting and writing each run on their own thread, handing pieces along by ownership
//! (nothing is copied to cross over) through short queues, so memory stays bounded and the total
//! time approaches that of the slowest of the three rather than their sum.
//!
//! ponytail: local files only. Reading URLs needs the shared live session (retries, live edge), so
//! it comes with that rather than as a second, simpler copy of it here.

use std::{
    error::Error,
    fs::File,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    rc::Rc,
    sync::mpsc::{SyncSender, sync_channel},
    thread,
};

use rstreamkit::{Continuous, Output, Transmuxer, hls, mkv, mp4, vod::Movie};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage: rstreamkit INPUT OUTPUT
  INPUT   MPEG-TS, an HLS playlist of local TS or fMP4 segments, MP4 or Matroska; - reads MPEG-TS from stdin
  OUTPUT  fragmented MP4; - writes it to stdout";

/// How much is read at a time. Measured: 1 MiB moves a 2 GB movie through in a fifth of the time
/// 4 MiB takes, because glibc hands buffers that big back to the system after each piece and every
/// new one is paid for again in page faults.
const CHUNK: u64 = 1 << 20;

/// Pieces waiting between threads. Measured: two keep every thread as busy as eight do, at
/// 17 MB of peak memory instead of 25 (a waiting piece can be a 2 MB group of pictures).
const QUEUE: usize = 2;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match run(input, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rstreamkit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &str, output: &str) -> Result {
    // INPUT is checked before OUTPUT is created, so a typo leaves no empty file behind; and
    // creating OUTPUT empties it, so it must not be INPUT under another name.
    if input != "-" {
        std::fs::metadata(input).map_err(|e| format!("{input}: {e}"))?;
    }
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(input), std::fs::canonicalize(output))
        && a == b
    {
        return Err("OUTPUT is the INPUT file; writing it would destroy what is being read".into());
    }
    let w: Box<dyn Write + Send> = if output == "-" {
        Box::new(io::stdout())
    } else {
        Box::new(File::create(output)?)
    };
    let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE);
    let writer = thread::spawn(move || {
        let mut w = BufWriter::with_capacity(1 << 20, w);
        for bytes in rx {
            w.write_all(&bytes)?;
        }
        w.flush()
    });
    let mut out = Out {
        tx,
        first_init: None,
        warned: false,
    };
    let converted = convert(input, &mut out);
    drop(out); // the writer finishes what it was given, then stops
    // A failed write (a closed pipe, a full disk) is the cause of whatever converting saw.
    writer.join().expect("the writing thread panicked")?;
    converted
}

/// Tells what INPUT is from its first bytes, not its name.
fn convert(input: &str, out: &mut Out) -> Result {
    if input == "-" {
        return stream(io::stdin(), out);
    }
    let mut file = File::open(input)?;
    let mut head = Vec::with_capacity(1024);
    (&mut file).take(1024).read_to_end(&mut head)?;
    file.rewind()?;
    if head.starts_with(b"#EXTM3U") {
        playlist(Path::new(input), out)
    } else if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        movie(file, true, out)
    } else if head.get(4..8) == Some(b"ftyp") {
        movie(file, false, out)
    } else if hls::looks_like_ts(&head) {
        stream(file, out)
    } else {
        Err("INPUT isn't MPEG-TS, an HLS playlist, MP4 or Matroska".into())
    }
}

/// MPEG-TS of any length, in bounded memory: it is cut into fragments at keyframes as it arrives,
/// while the next piece is already being read.
fn stream(input: impl Read + Send + 'static, out: &mut Out) -> Result {
    let (tx, rx) = sync_channel::<io::Result<Vec<u8>>>(QUEUE);
    thread::spawn(move || {
        let mut input = input;
        loop {
            let mut piece = Vec::with_capacity(CHUNK as usize);
            match (&mut input).take(CHUNK).read_to_end(&mut piece) {
                Ok(0) => return,
                // A closed queue means converting stopped (it failed): stop reading too.
                Ok(_) if tx.send(Ok(piece)).is_err() => return,
                Ok(_) => {}
                Err(e) => return drop(tx.send(Err(e))),
            }
        }
    });
    let mut ts = Continuous::default();
    for piece in rx {
        for o in ts.feed(&piece?)? {
            out.put(o)?;
        }
    }
    out.put(ts.finish()?)
}

/// An HLS playlist whose segments are local TS files; for a master playlist, its best variant.
fn playlist(path: &Path, out: &mut Out) -> Result {
    let dir = path.parent().unwrap_or(Path::new("."));
    let media = match hls::parse(&std::fs::read(path)?)? {
        hls::Parsed::Media(media) => media,
        hls::Parsed::Master(variants) => {
            let best = hls::pick_variant_for(&variants, u64::MAX).ok_or("no playable variant")?;
            return playlist(&local(dir, &best.uri)?, out);
        }
        _ => return Err("an HLS playlist of a kind this tool doesn't read".into()),
    };
    let mut t = Transmuxer::default();
    // fMP4 (CMAF) segments are already fragments: only their clocks are made continuous.
    let mut cmaf = rstreamkit::cmaf::Rebaser::default();
    let mut map_read = None;
    for (i, s) in media.segments.iter().enumerate() {
        // shortcut: no AES here; decrypting natively needs an AES implementation the crate doesn't
        // carry (the browser uses WebCrypto). Add one if encrypted recordings matter.
        if s.key.is_some() {
            return Err(format!("{}: encrypted segments aren't read here yet", s.uri).into());
        }
        let read = |uri: &str, range: &Option<std::ops::Range<u64>>| -> Result<Vec<u8>> {
            let path = local(dir, uri)?;
            Ok(match range {
                None => std::fs::read(&path)?,
                Some(r) => read_at(&mut File::open(&path)?, r.start, r.end - r.start)?,
            })
        };
        if i > 0 && map_read.is_some() != s.map.is_some() {
            return Err(format!("{}: a playlist mixing TS and fMP4 segments", s.uri).into());
        }
        let mut bytes = read(&s.uri, &s.byte_range)?;
        if let Some(map) = &s.map {
            if map_read.as_ref() != Some(map) {
                let init = read(&map.uri, &map.byte_range)?;
                cmaf.init(&init)?;
                out.init(init)?;
                map_read = Some(map.clone());
            }
            cmaf.fragment(&mut bytes, s.duration)?;
            out.send(bytes)?;
            continue;
        }
        if s.discontinuity {
            t.discontinuity();
        }
        out.put(t.push(&bytes)?)?;
    }
    Ok(())
}

fn local(dir: &Path, uri: &str) -> Result<PathBuf> {
    if uri.contains("://") {
        return Err(format!("{uri}: reading from the network isn't supported yet").into());
    }
    Ok(dir.join(uri))
}

/// An MP4 or Matroska file: its index is read first, then the frames, a piece at a time.
// ponytail: reading stays on this thread, because where the next piece is depends on the frames
// the last one held; writing is what runs alongside.
fn movie(mut file: File, matroska: bool, out: &mut Out) -> Result {
    let size = file.metadata()?.len();
    let movie = if matroska {
        let mut probe = mkv::Probe::new(size);
        let (mut at, mut len) = (0, 64 << 10);
        loop {
            match probe.feed(at, &read_at(&mut file, at, len)?)? {
                mkv::Step::Read(a, l) => (at, len) = (a, l),
                mkv::Step::Done(parsed) => break Movie::from_mkv(*parsed, size),
            }
        }
    } else {
        let mut at = 0;
        loop {
            match mp4::find_moov(&read_at(&mut file, at, 64)?, at) {
                mp4::Moov::At(offset, len) => {
                    let moov = read_at(&mut file, offset, len)?;
                    let (_, _, head) = mp4::box_header(&moov).ok_or("an unreadable moov box")?;
                    break Movie::from_mp4(&moov[head..], size)?;
                }
                mp4::Moov::Next(next) => at = next,
                mp4::Moov::Missing => return Err("an MP4 file without an index (moov)".into()),
            }
        }
    };
    let movie = Rc::new(movie);
    out.init(movie.init()?.bytes)?;
    let mut session = movie.session(0.0);
    // One buffer for every piece, refilled in place.
    let mut bytes = Vec::with_capacity(CHUNK as usize);
    while let Some((at, len)) = session.range(CHUNK) {
        file.seek(SeekFrom::Start(at))?;
        bytes.clear();
        (&mut file).take(len).read_to_end(&mut bytes)?;
        out.send(session.push(&bytes)?)?;
    }
    Ok(())
}

/// Up to `len` bytes from `at` (fewer at the end of the file).
fn read_at(file: &mut File, at: u64, len: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(at))?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The output file: one init segment, then fragments, handed to the writing thread.
struct Out {
    tx: SyncSender<Vec<u8>>,
    first_init: Option<Vec<u8>>,
    warned: bool,
}

impl Out {
    fn send(&mut self, bytes: Vec<u8>) -> Result {
        // The writer only hangs up when it failed; `run` reports why.
        Ok(self
            .tx
            .send(bytes)
            .map_err(|_| "the output stopped taking data")?)
    }

    fn put(&mut self, o: Output) -> Result {
        if let Some(init) = o.init {
            self.init(init.bytes)?;
        }
        for f in o.fragments {
            self.send(f.moof)?;
            self.send(f.mdat)?;
        }
        if let Some(codec) = &o.skipped_audio {
            self.warn(&format!("{codec} sound was left out"));
        }
        Ok(())
    }

    /// A file has one init segment. A browser takes a new one when a stream changes its codec
    /// settings; a file can't, so the change is reported instead.
    // ponytail: the first settings are kept; splitting the output into one file per setting is
    // the upgrade if recordings that change resolution matter.
    fn init(&mut self, bytes: Vec<u8>) -> Result {
        match &self.first_init {
            None => {
                self.first_init = Some(bytes.clone());
                self.send(bytes)?;
            }
            Some(first) if *first != bytes => self
                .warn("the stream changed its codec settings; players may not follow the change"),
            Some(_) => {}
        }
        Ok(())
    }

    fn warn(&mut self, what: &str) {
        if !std::mem::replace(&mut self.warned, true) {
            eprintln!("rstreamkit: warning: {what}");
        }
    }
}
