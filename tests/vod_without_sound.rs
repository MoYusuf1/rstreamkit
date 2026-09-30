//! Without the `sound` feature, a movie whose sound only our decoders could play is unsupported:
//! reported, not played silently. (With the feature, `vod.rs` covers playing it.)
#![cfg(not(feature = "sound"))]

use rstreamkit::{
    Unsupported, mp4,
    vod::{Movie, Verdict},
};

#[test]
fn sound_nobody_here_can_decode_makes_the_movie_unsupported() {
    let file: &[u8] = include_bytes!("fixtures/movie_ac3.mp4");
    let mp4::Moov::At(off, len) = mp4::find_moov(file, 0) else {
        panic!("no moov")
    };
    let (off, len) = (off as usize, len as usize);
    let (_, _, head) = mp4::box_header(&file[off..]).unwrap();
    let movie = Movie::from_mp4(&file[off + head..off + len], file.len() as u64).unwrap();

    let sound = movie.audio.as_ref().expect("the fixture has sound");
    // A browser that can't play the file itself either.
    assert_eq!(
        movie.verdict(&|_| false),
        Verdict::Unsupported(Unsupported::Sound(sound.name.clone()))
    );
}
