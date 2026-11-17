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

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Video length in seconds; the battle is sped up to fit.
    pub duration: f32,
    /// Hold the final frame (the result) for a moment.
    pub hold_end: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { width: 1080, height: 720, fps: 30, duration: 30.0, hold_end: true }
    }
}

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
        if !(1..=60).contains(&self.fps) || !(2.0..=600.0).contains(&self.duration) {
            return bad("fps must be 1–60 and duration 2–600 s");
        }
        Ok(())
    }
}

/// Renders `replay` to MP4 bytes.
pub fn render_mp4(
    replay: &[u8],
    data: &DataDir,
    opts: &RenderOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Vec<u8>, Summary), RenderError> {
    opts.validate()?;
    // Created on the first frame, so replay and data errors come first.
    let mut enc: Option<encode::Mp4Encoder> = None;
    let summary = scene::render(replay, data, opts, progress, &mut |pm| {
        let e = match &mut enc {
            Some(e) => e,
            None => enc.insert(
                encode::Mp4Encoder::new(opts.width, opts.height, opts.fps, encode::EncodeOptions::default())
                    .map_err(|e| RenderError::Encode(e.to_string()))?,
            ),
        };
        e.push_rgba(pm.data()).map_err(|e| RenderError::Encode(e.to_string()))
    })?;
    let enc = enc.ok_or_else(|| RenderError::Internal("no frames rendered".into()))?;
    progress(Progress { stage: Stage::Encode, done: 0, total: 1 });
    let mp4 = enc.finish().map_err(|e| RenderError::Encode(e.to_string()))?;
    progress(Progress { stage: Stage::Encode, done: 1, total: 1 });
    Ok((mp4, summary))
}
