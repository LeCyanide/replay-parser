//! Placeholder until the encoder lands.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct EncodeError(String);
#[derive(Default)]
pub struct EncodeOptions {
    pub bitrate_kbps: u32,
}
pub struct Mp4Encoder;
impl Mp4Encoder {
    pub fn new(_w: u32, _h: u32, _fps: u32, _o: EncodeOptions) -> Result<Self, EncodeError> {
        Err(EncodeError("encoder not built yet".into()))
    }
    pub fn push_rgba(&mut self, _rgba: &[u8]) -> Result<(), EncodeError> {
        Ok(())
    }
    pub fn finish(self) -> Result<Vec<u8>, EncodeError> {
        Ok(Vec::new())
    }
}
