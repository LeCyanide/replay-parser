//! Encodes N synthetic 1080x720@30 frames and reports wall time and size.
//!
//! cargo run --release --example bench -- [frames=900] [out.mp4] [bitrate_kbps] [qp] [baseline]
#[path = "../tests/support/synth.rs"]
mod synth;

use replay_encode::{EncodeOptions, Mp4Encoder};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: usize = args.get(1).map_or(900, |s| s.parse().unwrap());
    let out = args.get(2).cloned().unwrap_or_else(|| "bench.mp4".into());
    let mut opts = EncodeOptions::default();
    if let Some(b) = args.get(3) {
        opts.bitrate_kbps = b.parse().unwrap();
    }
    if let Some(q) = args.get(4) {
        opts.qp = q.parse().unwrap();
    }
    opts.baseline = args.get(5).is_some_and(|s| s == "baseline");
    let (w, h) = (1080, 720);

    // Render all frames up front so only the encoder is timed.
    let mut synth = synth::Synth::new(w, h);
    let rendered: Vec<Vec<u8>> = (0..frames)
        .map(|n| {
            let mut f = vec![0; w * h * 4];
            synth.render(n, &mut f);
            f
        })
        .collect();

    if let Ok(dump) = std::env::var("DUMP_RAW") {
        std::fs::write(dump, rendered.concat()).unwrap();
    }

    let start = Instant::now();
    let mut enc = Mp4Encoder::new(w as u32, h as u32, 30, opts.clone()).unwrap();
    for f in &rendered {
        enc.push_rgba(f).unwrap();
    }
    let mp4 = enc.finish().unwrap();
    let total = start.elapsed();
    std::fs::write(&out, &mp4).unwrap();
    println!(
        "{frames} frames {w}x{h}: {:.2} s total ({:.1} fps), {} bytes ({:.2} MB, {:.0} kbps) -> {out}  opts={opts:?}",
        total.as_secs_f64(),
        frames as f64 / total.as_secs_f64(),
        mp4.len(),
        mp4.len() as f64 / 1e6,
        mp4.len() as f64 * 8.0 / (frames as f64 / 30.0) / 1000.0,
    );
}
