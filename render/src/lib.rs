//! Renders a World of Warships replay to a minimap video with a battle
//! sidebar, on top of `replay_parser`'s battle simulation and draw commands.
//!
//! Layers:
//! - [`data`]: entity definitions, GameParams extract and map images from a
//!   data directory;
//! - [`scene`]: runs the battle and samples it into frames;
//! - [`map`] and [`sidebar`]: paint one frame;
//! - [`encode`]: frames to an H.264 MP4.

pub mod data;
pub mod encode;
pub mod map;
pub mod paint;
pub mod scene;
pub mod sidebar;
pub mod text;

pub use data::DataDir;
pub use scene::Summary;
pub use tiny_skia;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Playback speed against the battle: ×32 turns a 20-minute battle
    /// into 37.5 s. Ignored when `duration` is set.
    pub speed: f32,
    /// Fixed video length in seconds instead: the battle is sped up to fit.
    pub duration: Option<f32>,
    /// Hold the final frame (the result) for 1.5 s.
    pub hold_end: bool,
    /// Upper bound on the MP4's size in bytes; the bitrate is set from it
    /// and the video's length.
    pub max_bytes: u64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 1080,
            height: 720,
            fps: 30,
            speed: 32.0,
            duration: None,
            hold_end: true,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// 8 MB: under Discord's free upload limit (10 MB until August 2026, 20 MB
/// after, as reported), leaving room for a message.
pub const DEFAULT_MAX_BYTES: u64 = 8_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Parse,
    Simulate,
    Render,
    Encode,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Progress {
    pub stage: Stage,
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("not a readable replay: {0}")]
    BadReplay(String),
    #[error(transparent)]
    Data(#[from] data::DataError),
    #[error("encoding failed: {0}")]
    Encode(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the video would be {size} bytes, over the {max}-byte limit")]
    TooLarge { size: usize, max: u64 },
    #[error("{0}")]
    Internal(String),
}

impl RenderError {
    /// The CLI's error codes (see `bindings/node`).
    pub fn code(&self) -> &'static str {
        match self {
            RenderError::BadReplay(_) => "bad_replay",
            RenderError::Data(data::DataError::UnsupportedVersion(..)) => "unsupported_version",
            RenderError::Data(_) => "missing_data",
            RenderError::Io(_) => "io",
            RenderError::TooLarge { .. } => "too_large",
            RenderError::Encode(_) | RenderError::Internal(_) => "internal",
        }
    }
}

impl RenderOptions {
    pub fn validate(&self) -> Result<(), RenderError> {
        let bad = |m: &str| Err(RenderError::Internal(m.to_owned()));
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return bad("width and height must be even");
        }
        if self.height < 360 || self.width < self.height + 240 {
            return bad("the frame must be at least 360 px tall and leave 240 px beside the square map");
        }
        if !(1..=60).contains(&self.fps) {
            return bad("fps must be 1–60");
        }
        if !(1.0..=1000.0).contains(&self.speed) || self.duration.is_some_and(|d| !(2.0..=600.0).contains(&d)) {
            return bad("speed must be 1–1000 and duration 2–600 s");
        }
        if self.max_bytes < 200_000 {
            return bad("max size must be at least 200 kB");
        }
        Ok(())
    }
}

/// Share of the size budget the encoder may target: its rate control keeps
/// an average, so the first GOP can overshoot, and the container adds a
/// little.
const BUDGET_SHARE: f64 = 0.88;
/// Frames in flight between the renderer and the encoder thread.
const QUEUE: usize = 4;

/// Renders `replay` to MP4 bytes, under `opts.max_bytes`. Frames are encoded
/// on a second thread while the next ones render. If the result still comes
/// out too large (rare: the bitrate is an average), it is rendered once more
/// at a lower bitrate.
pub fn render_mp4(
    replay: &[u8],
    data: &DataDir,
    opts: &RenderOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Vec<u8>, Summary), RenderError> {
    opts.validate()?;
    let (mp4, summary) = encode_pass(replay, data, opts, 1.0, progress)?;
    if mp4.len() as u64 <= opts.max_bytes {
        return Ok((mp4, summary));
    }
    let scale = 0.7 * opts.max_bytes as f64 / mp4.len() as f64;
    let (mp4, mut summary) = encode_pass(replay, data, opts, scale, progress)?;
    if mp4.len() as u64 > opts.max_bytes {
        return Err(RenderError::TooLarge { size: mp4.len(), max: opts.max_bytes });
    }
    summary.warnings.push("re-encoded at a lower bitrate to fit the size limit".into());
    Ok((mp4, summary))
}

/// The bitrate (kbit/s) that fills `share` of `max_bytes` over `seconds`.
pub fn budget_kbps(max_bytes: u64, seconds: f32, share: f64) -> u32 {
    (max_bytes as f64 * 8.0 * share / seconds.max(1.0) as f64 / 1000.0).clamp(100.0, 50_000.0) as u32
}

struct ThreadedEncoder<'o> {
    opts: &'o RenderOptions,
    scale: f64,
    frames: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    spare: Option<std::sync::mpsc::Receiver<Vec<u8>>>,
    worker: Option<std::thread::JoinHandle<Result<Vec<u8>, RenderError>>>,
}

impl ThreadedEncoder<'_> {
    /// The encoder thread's error, if it stopped early.
    fn worker_error(&mut self) -> RenderError {
        self.frames = None;
        match self.worker.take().map(|w| w.join()) {
            Some(Ok(Err(e))) => e,
            Some(Err(_)) => RenderError::Internal("the encoder thread panicked".into()),
            _ => RenderError::Internal("the encoder thread stopped".into()),
        }
    }

    fn finish(mut self) -> Result<Vec<u8>, RenderError> {
        self.frames = None;
        match self.worker.take().map(|w| w.join()) {
            Some(Ok(r)) => r,
            Some(Err(_)) => Err(RenderError::Internal("the encoder thread panicked".into())),
            None => Err(RenderError::Internal("no frames rendered".into())),
        }
    }
}

impl scene::FrameSink for ThreadedEncoder<'_> {
    fn begin(&mut self, plan: &scene::Plan) -> Result<(), RenderError> {
        let kbps = (budget_kbps(self.opts.max_bytes, plan.seconds, BUDGET_SHARE) as f64 * self.scale) as u32;
        let options = encode::EncodeOptions { bitrate_kbps: kbps.max(100), ..encode::EncodeOptions::default() };
        let mut enc = encode::Mp4Encoder::new(self.opts.width, self.opts.height, self.opts.fps, options)
            .map_err(|e| RenderError::Encode(e.to_string()))?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(QUEUE);
        let (spare_tx, spare_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        self.frames = Some(tx);
        self.spare = Some(spare_rx);
        self.worker = Some(std::thread::spawn(move || {
            for buf in rx {
                enc.push_rgba(&buf).map_err(|e| RenderError::Encode(e.to_string()))?;
                // Hand the buffer back for reuse; the renderer may be gone.
                let _ = spare_tx.send(buf);
            }
            enc.finish().map_err(|e| RenderError::Encode(e.to_string()))
        }));
        Ok(())
    }

    fn frame(&mut self, pm: &tiny_skia::Pixmap) -> Result<(), RenderError> {
        let mut buf = self.spare.as_ref().and_then(|s| s.try_recv().ok()).unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(pm.data());
        let sent = self.frames.as_ref().map(|tx| tx.send(buf));
        match sent {
            Some(Ok(())) => Ok(()),
            _ => Err(self.worker_error()),
        }
    }
}

fn encode_pass(
    replay: &[u8],
    data: &DataDir,
    opts: &RenderOptions,
    scale: f64,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Vec<u8>, Summary), RenderError> {
    let mut sink = ThreadedEncoder { opts, scale, frames: None, spare: None, worker: None };
    let summary = scene::render(replay, data, opts, progress, &mut sink)?;
    progress(Progress { stage: Stage::Encode, done: 0, total: 1 });
    let mp4 = sink.finish()?;
    progress(Progress { stage: Stage::Encode, done: 1, total: 1 });
    Ok((mp4, summary))
}
