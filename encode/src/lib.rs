//! RGBA frames -> H.264 (OpenH264, statically linked) -> progressive MP4.
//!
//! ```no_run
//! # use replay_encode::{EncodeOptions, Mp4Encoder};
//! let mut enc = Mp4Encoder::new(1080, 720, 30, EncodeOptions::default())?;
//! # let frames: Vec<Vec<u8>> = vec![];
//! for rgba in &frames {
//!     enc.push_rgba(rgba)?;
//! }
//! let mp4_bytes: Vec<u8> = enc.finish()?;
//! # Ok::<(), replay_encode::EncodeError>(())
//! ```
//!
//! Output: H.264 Main profile (CABAC, no B-frames) or Constrained Baseline,
//! yuv420p, BT.709 limited range (signalled in both the SPS VUI and an MP4
//! `colr` box), constant frame rate, `moov` before `mdat`.

mod h264;
mod mp4;
mod yuv;

use std::fmt;

/// ISO/IEC 23091-2 colour code points; must match the matrix used in `yuv.rs`.
const COLOUR_PRIMARIES: u16 = 1; // BT.709
const TRANSFER_CHARACTERISTICS: u16 = 1; // BT.709
const MATRIX_COEFFICIENTS: u16 = 1; // BT.709

#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// Average bitrate ceiling. Content that fits at `qp` uses less; above it
    /// the quantiser is raised (up to `max_qp`) GOP by GOP to get back under.
    pub bitrate_kbps: u32,
    /// Quantiser used while under budget (0-51, lower = better and bigger).
    /// 23 keeps 1-px text crisp; each +6 roughly halves the size.
    pub qp: u8,
    /// Highest quantiser rate control may use. Beyond it, quality wins over
    /// the bitrate ceiling.
    pub max_qp: u8,
    /// Maximum distance between keyframes (IDR), in seconds. Must be >= 1.
    pub keyframe_interval_secs: u32,
    /// `false` (default): Main profile with CABAC (~10% smaller files).
    /// `true`: Constrained Baseline (CAVLC) for very old hardware decoders.
    pub baseline: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions { bitrate_kbps: 1000, qp: 23, max_qp: 40, keyframe_interval_secs: 2, baseline: false }
    }
}

#[derive(Debug)]
pub enum EncodeError {
    /// Width/height/fps/options not usable (e.g. odd dimensions).
    InvalidConfig(String),
    /// `push_rgba` buffer has the wrong length.
    BadFrameSize { expected: usize, actual: usize },
    /// `finish` called before any frame was pushed.
    NoFrames,
    /// Error reported by the H.264 encoder, or inconsistent encoder output.
    Codec(String),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::InvalidConfig(m) => write!(f, "invalid encoder configuration: {m}"),
            EncodeError::BadFrameSize { expected, actual } => {
                write!(f, "RGBA frame has {actual} bytes, expected {expected}")
            }
            EncodeError::NoFrames => write!(f, "no frames were encoded"),
            EncodeError::Codec(m) => write!(f, "H.264 encoder error: {m}"),
        }
    }
}

impl std::error::Error for EncodeError {}

pub struct Mp4Encoder {
    width: usize,
    height: usize,
    fps: u32,
    keyframe_interval: u64,
    frame_index: u64,
    frames_since_idr: u64,
    yuv: Vec<u8>, // I420 scratch buffer, reused for every frame
    rate: RateControl,
    encoder: h264::Encoder,
    mux: mp4::Mp4Writer,
}

impl Mp4Encoder {
    pub fn new(width: u32, height: u32, fps: u32, opts: EncodeOptions) -> Result<Self, EncodeError> {
        let invalid = |m: String| Err(EncodeError::InvalidConfig(m));
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return invalid(format!("dimensions must be non-zero and even, got {width}x{height}"));
        }
        if width.max(height) > 3840 || width.min(height) > 2160 {
            return invalid(format!("{width}x{height} exceeds OpenH264's 3840x2160 limit"));
        }
        if !(1..=60).contains(&fps) {
            return invalid(format!("fps must be in 1..=60, got {fps}"));
        }
        if opts.keyframe_interval_secs == 0 {
            return invalid("keyframe_interval_secs must be >= 1".into());
        }
        if opts.qp > opts.max_qp || opts.max_qp > 51 {
            return invalid(format!("need qp <= max_qp <= 51, got {} and {}", opts.qp, opts.max_qp));
        }
        if opts.bitrate_kbps == 0 {
            return invalid("bitrate_kbps must be > 0".into());
        }

        let keyframe_interval = opts.keyframe_interval_secs as u64 * fps as u64;
        let encoder = h264::Encoder::new(&h264::Config { width, height, fps, qp: opts.qp, cabac: !opts.baseline })?;
        let (w, h) = (width as usize, height as usize);
        Ok(Mp4Encoder {
            width: w,
            height: h,
            fps,
            keyframe_interval,
            frame_index: 0,
            frames_since_idr: 0,
            yuv: vec![0; w * h * 3 / 2],
            rate: RateControl::new(&opts, fps, keyframe_interval),
            encoder,
            mux: mp4::Mp4Writer::new(width, height, fps),
        })
    }

    /// Tightly packed RGBA8, width*height*4 bytes, top row first (tiny-skia Pixmap
    /// layout, premultiplied alpha but frames are opaque). Alpha is ignored.
    pub fn push_rgba(&mut self, rgba: &[u8]) -> Result<(), EncodeError> {
        let expected = self.width * self.height * 4;
        if rgba.len() != expected {
            return Err(EncodeError::BadFrameSize { expected, actual: rgba.len() });
        }
        let luma_len = self.width * self.height;
        let (y, uv) = self.yuv.split_at_mut(luma_len);
        let (u, v) = uv.split_at_mut(luma_len / 4);
        yuv::rgba_to_i420(rgba, self.width, self.height, y, u, v);

        let force_idr = self.frame_index == 0 || self.frames_since_idr >= self.keyframe_interval;
        self.encoder.set_qp(self.rate.qp_for_next_frame(force_idr))?;
        let timestamp_ms = (self.frame_index * 1000 / self.fps as u64) as i64;
        let frame = self.encoder.encode([y, u, v], timestamp_ms, force_idr)?;
        if force_idr && !frame.is_idr {
            return Err(EncodeError::Codec("encoder ignored keyframe request".into()));
        }
        // The encoder may also insert IDRs itself on scene changes.
        self.frames_since_idr = if frame.is_idr { 1 } else { self.frames_since_idr + 1 };
        let bytes = self.mux.add_sample(frame.nal_units(), frame.is_idr)?;
        self.rate.frame_done(bytes);
        self.frame_index += 1;
        Ok(())
    }

    /// Complete MP4 file bytes.
    pub fn finish(self) -> Result<Vec<u8>, EncodeError> {
        if self.frame_index == 0 {
            return Err(EncodeError::NoFrames);
        }
        self.mux.finish()
    }
}

/// GOP-level rate control: the QP is normally constant within a GOP (no
/// quality pulsing, keyframes as sharp as the rest) and re-chosen at each
/// forced IDR so the running average converges to at most `bitrate_kbps`.
/// If the overspend exceeds a whole GOP's budget it also re-plans mid-GOP
/// (at most every half second), so a sudden complex scene can't run away.
struct RateControl {
    base_qp: u8,
    max_qp: u8,
    qp: u8,
    bits_per_frame: f64,
    frames_per_gop: f64,
    min_replan_frames: u64,
    frames: u64,
    spent_bits: f64,
    /// Frames/bits since the last re-plan.
    window_frames: u64,
    window_bits: f64,
}

impl RateControl {
    fn new(opts: &EncodeOptions, fps: u32, keyframe_interval: u64) -> Self {
        RateControl {
            base_qp: opts.qp,
            max_qp: opts.max_qp,
            qp: opts.qp,
            bits_per_frame: opts.bitrate_kbps as f64 * 1000.0 / fps as f64,
            frames_per_gop: keyframe_interval as f64,
            min_replan_frames: (fps as u64 / 2).max(1),
            frames: 0,
            spent_bits: 0.0,
            window_frames: 0,
            window_bits: 0.0,
        }
    }

    fn frame_done(&mut self, bytes: usize) {
        let bits = bytes as f64 * 8.0;
        self.frames += 1;
        self.spent_bits += bits;
        self.window_frames += 1;
        self.window_bits += bits;
    }

    fn qp_for_next_frame(&mut self, keyframe: bool) -> u8 {
        let gop_budget = self.bits_per_frame * self.frames_per_gop;
        // Positive = under budget so far, negative = overspent.
        let balance = self.bits_per_frame * self.frames as f64 - self.spent_bits;
        let runaway = -balance > gop_budget && self.window_frames >= self.min_replan_frames;
        if !(keyframe || runaway) {
            return self.qp;
        }
        if self.window_frames > 0 {
            // Budget for the next GOP: the nominal share, plus half of any
            // surplus (or minus half of any debt) accumulated so far.
            let target = (gop_budget + balance / 2.0).max(gop_budget / 4.0);
            // Predict the next GOP costs what the recent window did at the same
            // QP; in H.264, +6 QP roughly halves the bits.
            let predicted = self.window_bits * self.frames_per_gop / self.window_frames as f64;
            let delta = (6.0 * (predicted / target).log2()).round().clamp(-6.0, 12.0) as i32;
            self.qp = (self.qp as i32 + delta).clamp(self.base_qp as i32, self.max_qp as i32) as u8;
        }
        self.window_frames = 0;
        self.window_bits = 0.0;
        self.qp
    }
}
