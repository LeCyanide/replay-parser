//! Tiny progressive ("faststart") MP4 writer for a single constant-frame-rate
//! H.264 track without B-frames: `ftyp`, `moov`, `mdat`, in that order.
//!
//! All samples go into one chunk, so the sample table is just `stts` (one
//! run), `stss`, `stsc` (one entry), `stsz` and `stco` (one offset). No `ctts`
//! is needed because decode order equals presentation order.

use crate::EncodeError;

pub struct Mp4Writer {
    width: u32,
    height: u32,
    timescale: u32,
    sample_delta: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    mdat: Vec<u8>,
    sizes: Vec<u32>,
    sync_samples: Vec<u32>, // 1-based sample numbers
}

impl Mp4Writer {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Mp4Writer {
            width,
            height,
            // 1/fps per sample, with enough resolution that rounding in tools stays exact.
            timescale: fps * 1000,
            sample_delta: 1000,
            sps: None,
            pps: None,
            mdat: Vec::new(),
            sizes: Vec::new(),
            sync_samples: Vec::new(),
        }
    }

    /// Adds one access unit. SPS/PPS NAL units are moved into `avcC`; the rest
    /// are stored length-prefixed (4-byte big-endian) as one sample. Returns its size.
    pub fn add_sample<'a>(
        &mut self,
        nals: impl Iterator<Item = &'a [u8]>,
        is_sync: bool,
    ) -> Result<usize, EncodeError> {
        let start = self.mdat.len();
        for nal in nals {
            let Some(&header) = nal.first() else { continue };
            let param_set = match header & 0x1f {
                7 => Some(&mut self.sps),
                8 => Some(&mut self.pps),
                _ => None,
            };
            if let Some(slot) = param_set {
                match slot {
                    None => *slot = Some(nal.to_vec()),
                    Some(prev) if prev.as_slice() == nal => {}
                    Some(_) => return Err(EncodeError::Codec("parameter sets changed mid-stream".into())),
                }
                continue;
            }
            self.mdat.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            self.mdat.extend_from_slice(nal);
        }
        let size = self.mdat.len() - start;
        if size == 0 {
            return Err(EncodeError::Codec("encoder produced an empty access unit".into()));
        }
        self.sizes.push(size as u32);
        if is_sync {
            self.sync_samples.push(self.sizes.len() as u32);
        }
        Ok(size)
    }

    pub fn finish(self) -> Result<Vec<u8>, EncodeError> {
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return Err(EncodeError::NoFrames);
        };
        if sps.len() < 4 {
            return Err(EncodeError::Codec("malformed SPS".into()));
        }
        if self.sync_samples.first() != Some(&1) {
            return Err(EncodeError::Codec("first frame is not a keyframe".into()));
        }

        let ftyp = ftyp();
        // moov precedes mdat, so stco depends on moov's size. That size does not
        // depend on the offset value: build once to measure, then for real.
        let mut moov = self.moov(sps, pps, 0);
        let mdat_header = 8;
        let data_offset = ftyp.len() + moov.len() + mdat_header;
        if data_offset + self.mdat.len() > u32::MAX as usize {
            return Err(EncodeError::Codec("output exceeds 4 GiB".into()));
        }
        moov = self.moov(sps, pps, data_offset as u32);

        let mut out = Vec::with_capacity(data_offset + self.mdat.len());
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        out.extend_from_slice(&((self.mdat.len() + mdat_header) as u32).to_be_bytes());
        out.extend_from_slice(b"mdat");
        out.extend_from_slice(&self.mdat);
        Ok(out)
    }

    fn moov(&self, sps: &[u8], pps: &[u8], chunk_offset: u32) -> Vec<u8> {
        let n = self.sizes.len() as u32;
        let media_duration = n as u64 * self.sample_delta as u64;
        const MOVIE_TIMESCALE: u32 = 1000;
        let movie_duration = (media_duration * MOVIE_TIMESCALE as u64 / self.timescale as u64) as u32;

        let mvhd = full_box(b"mvhd", 0, 0, |b| {
            b.u32(0).u32(0); // creation / modification time
            b.u32(MOVIE_TIMESCALE).u32(movie_duration);
            b.u32(0x0001_0000).u16(0x0100).zeros(10); // rate 1.0, volume 1.0, reserved
            b.matrix();
            b.zeros(24); // pre_defined
            b.u32(2); // next_track_ID
        });
        let tkhd = full_box(b"tkhd", 0, 0x3, |b| {
            // flags: track_enabled | track_in_movie
            b.u32(0).u32(0).u32(1).u32(0).u32(movie_duration); // times, track_ID, reserved, duration
            b.zeros(8).u16(0).u16(0).u16(0).u16(0); // reserved, layer, alternate_group, volume, reserved
            b.matrix();
            b.u32(self.width << 16).u32(self.height << 16);
        });
        let mdhd = full_box(b"mdhd", 0, 0, |b| {
            b.u32(0).u32(0).u32(self.timescale).u32(media_duration as u32);
            b.u16(0x55c4).u16(0); // language "und", pre_defined
        });
        let hdlr = full_box(b"hdlr", 0, 0, |b| {
            b.u32(0).bytes(b"vide").zeros(12).bytes(b"VideoHandler\0");
        });
        let vmhd = full_box(b"vmhd", 0, 1, |b| {
            b.zeros(8); // graphicsmode, opcolor
        });
        let url = full_box(b"url ", 0, 1, |_| {}); // flag 1: media is in this file
        let dref = full_box(b"dref", 0, 0, |b| {
            b.u32(1).bytes(&url);
        });
        let dinf = plain_box(b"dinf", |b| {
            b.bytes(&dref);
        });

        let avcc = plain_box(b"avcC", |b| {
            b.u8(1).u8(sps[1]).u8(sps[2]).u8(sps[3]); // version, profile, compat, level
            b.u8(0xfc | 3); // 4-byte NAL length prefixes
            b.u8(0xe0 | 1).u16(sps.len() as u16).bytes(sps);
            b.u8(1).u16(pps.len() as u16).bytes(pps);
        });
        let colr = plain_box(b"colr", |b| {
            b.bytes(b"nclx");
            b.u16(crate::COLOUR_PRIMARIES).u16(crate::TRANSFER_CHARACTERISTICS).u16(crate::MATRIX_COEFFICIENTS);
            b.u8(0); // full_range_flag = 0 (limited)
        });
        let avc1 = plain_box(b"avc1", |b| {
            b.zeros(6).u16(1); // reserved, data_reference_index
            b.zeros(16); // pre_defined, reserved
            b.u16(self.width as u16).u16(self.height as u16);
            b.u32(0x0048_0000).u32(0x0048_0000).u32(0).u16(1); // 72 dpi, reserved, frame_count
            b.zeros(32); // compressorname
            b.u16(0x0018).u16(0xffff); // depth, pre_defined = -1
            b.bytes(&avcc).bytes(&colr);
        });
        let stsd = full_box(b"stsd", 0, 0, |b| {
            b.u32(1).bytes(&avc1);
        });
        let stts = full_box(b"stts", 0, 0, |b| {
            b.u32(1).u32(n).u32(self.sample_delta);
        });
        let stss = full_box(b"stss", 0, 0, |b| {
            b.u32(self.sync_samples.len() as u32);
            self.sync_samples.iter().for_each(|&s| _ = b.u32(s));
        });
        let stsc = full_box(b"stsc", 0, 0, |b| {
            b.u32(1).u32(1).u32(n).u32(1); // one chunk holding all samples
        });
        let stsz = full_box(b"stsz", 0, 0, |b| {
            b.u32(0).u32(n);
            self.sizes.iter().for_each(|&s| _ = b.u32(s));
        });
        let stco = full_box(b"stco", 0, 0, |b| {
            b.u32(1).u32(chunk_offset);
        });
        let stbl = plain_box(b"stbl", |b| {
            b.bytes(&stsd).bytes(&stts).bytes(&stss).bytes(&stsc).bytes(&stsz).bytes(&stco);
        });
        let minf = plain_box(b"minf", |b| {
            b.bytes(&vmhd).bytes(&dinf).bytes(&stbl);
        });
        let mdia = plain_box(b"mdia", |b| {
            b.bytes(&mdhd).bytes(&hdlr).bytes(&minf);
        });
        let trak = plain_box(b"trak", |b| {
            b.bytes(&tkhd).bytes(&mdia);
        });
        plain_box(b"moov", |b| {
            b.bytes(&mvhd).bytes(&trak);
        })
    }
}

fn ftyp() -> Vec<u8> {
    plain_box(b"ftyp", |b| {
        b.bytes(b"isom").u32(0x200);
        b.bytes(b"isom").bytes(b"iso2").bytes(b"avc1").bytes(b"mp41");
    })
}

/// Big-endian byte writer used to fill box payloads.
struct Buf(Vec<u8>);

impl Buf {
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes(&[v])
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_be_bytes())
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_be_bytes())
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
    /// Identity transformation matrix (16.16 / 2.30 fixed point).
    fn matrix(&mut self) -> &mut Self {
        for v in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            self.u32(v);
        }
        self
    }
}

fn plain_box(kind: &[u8; 4], fill: impl FnOnce(&mut Buf)) -> Vec<u8> {
    let mut b = Buf(Vec::new());
    b.u32(0).bytes(kind);
    fill(&mut b);
    let size = b.0.len() as u32;
    b.0[..4].copy_from_slice(&size.to_be_bytes());
    b.0
}

fn full_box(kind: &[u8; 4], version: u8, flags: u32, fill: impl FnOnce(&mut Buf)) -> Vec<u8> {
    plain_box(kind, |b| {
        b.u32((version as u32) << 24 | flags);
        fill(b);
    })
}
