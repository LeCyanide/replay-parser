#[path = "support/synth.rs"]
mod synth;

use replay_encode::{EncodeError, EncodeOptions, Mp4Encoder};
use std::process::Command;

/// Top-level boxes as (type, offset, size).
fn top_level_boxes(mp4: &[u8]) -> Vec<(String, usize, usize)> {
    let mut boxes = Vec::new();
    let mut pos = 0;
    while pos + 8 <= mp4.len() {
        let size = u32::from_be_bytes(mp4[pos..pos + 4].try_into().unwrap()) as usize;
        boxes.push((String::from_utf8_lossy(&mp4[pos + 4..pos + 8]).into_owned(), pos, size));
        assert!(size >= 8, "bad box size");
        pos += size;
    }
    assert_eq!(pos, mp4.len(), "boxes must exactly cover the file");
    boxes
}

fn find_box<'a>(data: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let i = data.windows(4).position(|w| w == kind).unwrap_or_else(|| panic!("no {kind:?} box"));
    let size = u32::from_be_bytes(data[i - 4..i].try_into().unwrap()) as usize;
    &data[i + 4..i - 4 + size]
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(b[i..i + 4].try_into().unwrap())
}

fn encode_synthetic(frames: usize, width: usize, height: usize, opts: EncodeOptions) -> Vec<u8> {
    let mut synth = synth::Synth::new(width, height);
    let mut rgba = vec![0u8; width * height * 4];
    let mut enc = Mp4Encoder::new(width as u32, height as u32, 30, opts).unwrap();
    for n in 0..frames {
        synth.render(n, &mut rgba);
        enc.push_rgba(&rgba).unwrap();
    }
    enc.finish().unwrap()
}

#[test]
fn encodes_300_frames_with_valid_structure() {
    let mp4 = encode_synthetic(300, 1080, 720, EncodeOptions::default());
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("synthetic_300.mp4");
    std::fs::write(&path, &mp4).unwrap();
    eprintln!("wrote {} ({} bytes)", path.display(), mp4.len());

    let order: Vec<_> = top_level_boxes(&mp4).into_iter().map(|b| b.0).collect();
    assert_eq!(order, ["ftyp", "moov", "mdat"]);

    let stsz = find_box(&mp4, b"stsz");
    assert_eq!(u32_at(stsz, 8), 300, "sample count");
    let stss = find_box(&mp4, b"stss");
    let keyframes: Vec<u32> = (0..u32_at(stss, 4) as usize).map(|i| u32_at(stss, 8 + 4 * i)).collect();
    assert_eq!(keyframes[0], 1);
    assert!(keyframes.windows(2).all(|k| k[1] - k[0] <= 60), "IDR at least every 2 s: {keyframes:?}");

    // stco must point at the first sample, i.e. right after the mdat header.
    let stco = find_box(&mp4, b"stco");
    let mdat_offset = top_level_boxes(&mp4)[2].1;
    assert_eq!(u32_at(stco, 8) as usize, mdat_offset + 8);
    // First sample's first NAL is an IDR slice (type 5), 4-byte length-prefixed.
    let first = mdat_offset + 8;
    assert_eq!(mp4[first + 4] & 0x1f, 5);

    if let Some(report) = ffprobe(&path) {
        eprintln!("{report}");
        for expect in [
            "codec_name=h264",
            "profile=Main",
            "pix_fmt=yuv420p",
            "width=1080",
            "height=720",
            "r_frame_rate=30/1",
            "avg_frame_rate=30/1",
            "nb_frames=300",
            "color_range=tv",
            "color_space=bt709",
            "color_primaries=bt709",
            "color_transfer=bt709",
            "duration=10.000000",
        ] {
            assert!(report.contains(expect), "ffprobe output lacks {expect}");
        }
        let decode =
            Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-f", "null", "-"]).output().unwrap();
        assert!(decode.status.success());
        assert!(decode.stderr.is_empty(), "decode errors: {}", String::from_utf8_lossy(&decode.stderr));
    }
}

#[test]
fn rate_control_holds_bitrate_ceiling() {
    // The synthetic map needs ~800 kbps at the default QP; ask for 300 kbps.
    let opts = EncodeOptions { bitrate_kbps: 300, ..Default::default() };
    let mp4 = encode_synthetic(300, 1080, 720, opts);
    let budget = 300_000 / 8 * 10; // 10 s
    eprintln!("size {} vs budget {budget}", mp4.len());
    assert!(mp4.len() < budget * 115 / 100, "{} bytes exceeds 300 kbps budget", mp4.len());
    assert!(mp4.len() > budget * 60 / 100, "{} bytes: rate control far too conservative", mp4.len());
}

#[test]
fn baseline_option_produces_constrained_baseline() {
    let opts = EncodeOptions { baseline: true, ..Default::default() };
    let mp4 = encode_synthetic(10, 320, 240, opts);
    let avcc = find_box(&mp4, b"avcC");
    assert_eq!(avcc[1], 66, "profile_idc");
    assert!(avcc[2] & 0x40 != 0, "constraint_set1_flag (constrained baseline)");
}

#[test]
fn rejects_bad_input() {
    let odd = Mp4Encoder::new(1081, 720, 30, EncodeOptions::default());
    assert!(matches!(odd, Err(EncodeError::InvalidConfig(_))));
    let mut enc = Mp4Encoder::new(64, 64, 30, EncodeOptions::default()).unwrap();
    assert!(matches!(enc.push_rgba(&[0; 10]), Err(EncodeError::BadFrameSize { .. })));
    assert!(matches!(enc.finish(), Err(EncodeError::NoFrames)));
}

/// `ffprobe -show_streams -show_format` output, or None if ffprobe isn't installed.
fn ffprobe(path: &std::path::Path) -> Option<String> {
    let out = Command::new("ffprobe").args(["-v", "error", "-show_streams", "-show_format"]).arg(path).output().ok()?;
    assert!(out.status.success(), "ffprobe failed: {}", String::from_utf8_lossy(&out.stderr));
    Some(String::from_utf8(out.stdout).unwrap())
}
