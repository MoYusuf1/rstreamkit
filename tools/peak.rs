//! `peak COMMAND [ARGS...]`: runs COMMAND and prints its wall time and peak memory as JSON.
//!
//! Linux counts a process's peak memory from before it starts the program, while it is still a
//! copy of whatever spawned it. Spawned from Python holding hundreds of megabytes, every program
//! looks that big. This launcher is tiny, so what it reports is the program's own peak.
//!   rustc -O tools/peak.rs -o target/peak
use std::{
    process::{Command, Stdio},
    time::Instant,
};

/// `struct rusage` on 64-bit Linux: two timevals, then fourteen longs, max RSS (KiB) first.
#[repr(C)]
struct Rusage {
    times: [i64; 4],
    max_rss_kib: i64,
    rest: [i64; 13],
}

unsafe extern "C" {
    fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut Rusage) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let started = Instant::now();
    let child = Command::new(&args[0])
        .args(&args[1..])
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut status = 0;
    let mut usage = Rusage { times: [0; 4], max_rss_kib: 0, rest: [0; 13] };
    // SAFETY: the pid is our own child, and both pointers are to live, correctly laid out values.
    if unsafe { wait4(child.id() as i32, &mut status, 0, &mut usage) } < 0 {
        panic!("wait4 failed");
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    println!("{{\"ms\":{ms:.3},\"peak_bytes\":{},\"status\":{status}}}", usage.max_rss_kib * 1024);
    std::process::exit(if status == 0 { 0 } else { 1 });
}
