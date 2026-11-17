//! Minimal safe wrapper around the OpenH264 encoder (statically compiled from
//! Cisco's sources by `openh264-sys2`).
//!
//! We use the raw `-sys` bindings instead of the higher-level `openh264` crate
//! because that crate exposes neither CABAC nor fixed-QP mode with runtime QP
//! changes, and it pulls in the decoder and its own colour conversion.

use crate::EncodeError;
use openh264_sys2 as sys;
use std::os::raw::{c_int, c_void};
use std::ptr;

pub struct Config {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub qp: u8,
    pub cabac: bool,
}

/// One encoded access unit.
pub struct Frame<'a> {
    pub is_idr: bool,
    info: &'a sys::SFrameBSInfo,
}

impl<'a> Frame<'a> {
    /// The NAL units of this access unit, start codes stripped.
    pub fn nal_units(&self) -> impl Iterator<Item = &'a [u8]> + 'a {
        let info = self.info;
        info.sLayerInfo[..info.iLayerNum as usize].iter().flat_map(|layer| {
            // SAFETY: OpenH264 guarantees `iNalCount` lengths, laid out back to
            // back in `pBsBuf`; valid until the next EncodeFrame call, which the
            // `&'a` borrow of the encoder prevents.
            let lens = unsafe { std::slice::from_raw_parts(layer.pNalLengthInByte, layer.iNalCount as usize) };
            let total = lens.iter().map(|&l| l as usize).sum();
            let mut buf = unsafe { std::slice::from_raw_parts(layer.pBsBuf, total) };
            lens.iter().map(move |&len| {
                let (nal, rest) = buf.split_at(len as usize);
                buf = rest;
                match nal {
                    [0, 0, 0, 1, payload @ ..] | [0, 0, 1, payload @ ..] => payload,
                    _ => nal,
                }
            })
        })
    }
}

pub struct Encoder {
    enc: *mut sys::ISVCEncoder,
    vtbl: sys::ISVCEncoderVtbl,
    params: Box<sys::SEncParamExt>, // kept to change the QP at runtime
    info: Box<sys::SFrameBSInfo>,   // ~10 KB output descriptor, reused
}

// SAFETY: the OpenH264 encoder instance is not tied to the creating thread.
unsafe impl Send for Encoder {}

fn check(ret: c_int, what: &str) -> Result<(), EncodeError> {
    if ret == sys::cmResultSuccess as c_int {
        Ok(())
    } else {
        Err(EncodeError::Codec(format!("{what} failed (OpenH264 code {ret})")))
    }
}

impl Encoder {
    pub fn new(cfg: &Config) -> Result<Self, EncodeError> {
        let mut enc: *mut sys::ISVCEncoder = ptr::null_mut();
        // SAFETY: plain FFI constructor; result checked below.
        check(unsafe { sys::source::APILoader::WelsCreateSVCEncoder(&mut enc) }, "WelsCreateSVCEncoder")?;
        if enc.is_null() {
            return Err(EncodeError::Codec("WelsCreateSVCEncoder returned null".into()));
        }
        // SAFETY: `enc` points to a valid encoder whose first field is its vtable.
        let vtbl = unsafe { ptr::read(*enc) };
        let mut this = Encoder { enc, vtbl, params: Box::default(), info: Box::default() };
        this.set_trace_quiet();
        // SAFETY: valid encoder and out-pointer.
        check(unsafe { (this.vtbl.GetDefaultParams.unwrap())(enc, &mut *this.params) }, "GetDefaultParams")?;
        configure(&mut this.params, cfg);
        // SAFETY: params fully initialised by GetDefaultParams + configure.
        check(unsafe { (this.vtbl.InitializeExt.unwrap())(enc, &*this.params) }, "InitializeExt")?;
        this.set_trace_quiet();
        let mut fmt: c_int = sys::videoFormatI420 as c_int;
        this.set_option(sys::ENCODER_OPTION_DATAFORMAT, &mut fmt, "SetOption(DATAFORMAT)")?;
        Ok(this)
    }

    fn set_option<T>(&mut self, option: sys::ENCODER_OPTION, value: &mut T, what: &str) -> Result<(), EncodeError> {
        // SAFETY: callers pass the value type OpenH264 documents for `option`.
        let ret = unsafe { (self.vtbl.SetOption.unwrap())(self.enc, option, (value as *mut T).cast::<c_void>()) };
        check(ret, what)
    }

    fn set_trace_quiet(&mut self) {
        let mut level: c_int = sys::WELS_LOG_QUIET as c_int;
        // Best effort: before initialisation this may report failure but still applies.
        let _ = self.set_option(sys::ENCODER_OPTION_TRACE_LEVEL, &mut level, "SetOption(TRACE_LEVEL)");
    }

    /// Changes the quantiser for subsequent frames (no encoder reset, no new IDR).
    pub fn set_qp(&mut self, qp: u8) -> Result<(), EncodeError> {
        if self.params.sSpatialLayers[0].iDLayerQp != qp as c_int {
            self.params.sSpatialLayers[0].iDLayerQp = qp as c_int;
            let mut params = *self.params;
            self.set_option(sys::ENCODER_OPTION_SVC_ENCODE_PARAM_EXT, &mut params, "SetOption(PARAM_EXT)")?;
        }
        Ok(())
    }

    /// Encodes one I420 picture (tightly packed planes).
    pub fn encode(&mut self, planes: [&[u8]; 3], timestamp_ms: i64, force_idr: bool) -> Result<Frame<'_>, EncodeError> {
        let (w, h) = (self.params.iPicWidth, self.params.iPicHeight);
        let luma = (w * h) as usize;
        assert!(planes[0].len() == luma && planes[1].len() == luma / 4 && planes[2].len() == luma / 4);
        let pic = sys::SSourcePicture {
            iColorFormat: sys::videoFormatI420 as c_int,
            iStride: [w, w / 2, w / 2, 0],
            // OpenH264 only reads the input planes; the API just isn't const-correct.
            pData: [
                planes[0].as_ptr().cast_mut(),
                planes[1].as_ptr().cast_mut(),
                planes[2].as_ptr().cast_mut(),
                ptr::null_mut(),
            ],
            iPicWidth: w,
            iPicHeight: h,
            uiTimeStamp: timestamp_ms,
            bPsnrY: false,
            bPsnrU: false,
            bPsnrV: false,
        };
        // SAFETY: `pic` describes live buffers of the configured size; `info` is
        // owned by us and only read through the returned borrow.
        unsafe {
            if force_idr {
                check((self.vtbl.ForceIntraFrame.unwrap())(self.enc, true), "ForceIntraFrame")?;
            }
            check((self.vtbl.EncodeFrame.unwrap())(self.enc, &pic, &mut *self.info), "EncodeFrame")?;
        }
        let frame_type = self.info.eFrameType;
        if frame_type == sys::videoFrameTypeSkip || frame_type == sys::videoFrameTypeInvalid {
            return Err(EncodeError::Codec(format!("encoder produced no picture (frame type {frame_type})")));
        }
        Ok(Frame { is_idr: frame_type == sys::videoFrameTypeIDR, info: &self.info })
    }
}

fn configure(p: &mut sys::SEncParamExt, cfg: &Config) {
    let (w, h) = (cfg.width as c_int, cfg.height as c_int);
    // Screen-content tools gave ~5% smaller files at ~1 dB higher PSNR than
    // camera mode on minimap-like content (for ~1.4x encode time). The
    // "non real time" usage types are rejected by this OpenH264 version.
    p.iUsageType = sys::SCREEN_CONTENT_REAL_TIME;
    p.iPicWidth = w;
    p.iPicHeight = h;
    p.fMaxFrameRate = cfg.fps as f32;
    // Fixed QP; `crate::RateControl` adjusts it between GOPs. OpenH264's own
    // bitrate modes are tuned for real-time calls: they coded our keyframes at
    // QP 32+ (blurry text every 2 s) while undershooting the target.
    p.iRCMode = sys::RC_OFF_MODE;
    p.bEnableFrameSkip = false; // every pushed frame must become exactly one sample
    p.iComplexityMode = sys::HIGH_COMPLEXITY;
    p.iEntropyCodingModeFlag = cfg.cabac.into();
    p.iTemporalLayerNum = 1;
    p.iSpatialLayerNum = 1;
    p.iNumRefFrame = 1;
    p.uiIntraPeriod = 0; // IDRs are forced by the caller on a fixed cadence
    p.bEnableSceneChangeDetect = true; // required by screen-content mode
    p.bEnableLongTermReference = false;
    p.bEnableBackgroundDetection = false; // unsupported for screen content
    p.bEnableAdaptiveQuant = false; // unsupported for screen content
    p.bEnableDenoise = false;
    p.eSpsPpsIdStrategy = sys::CONSTANT_ID; // identical SPS/PPS every IDR -> one avcC entry
    p.bPrefixNalAddingCtrl = false;
    p.bEnableSSEI = false;
    // Slice-parallel threading measured slower here (the screen-content analysis
    // is single-threaded), so: one thread, one slice, deterministic output.
    p.iMultipleThreadIdc = 1;
    p.iLoopFilterDisableIdc = 0; // deblocking on

    let layer = &mut p.sSpatialLayers[0];
    layer.iVideoWidth = w;
    layer.iVideoHeight = h;
    layer.fFrameRate = cfg.fps as f32;
    layer.iDLayerQp = cfg.qp.into();
    // Main = CABAC (smaller); Baseline is emitted as Constrained Baseline.
    layer.uiProfileIdc = if cfg.cabac { sys::PRO_MAIN } else { sys::PRO_BASELINE };
    layer.uiLevelIdc = 0; // auto (3.1 for 1080x720@30)
    layer.sSliceArgument.uiSliceMode = sys::SM_SINGLE_SLICE;
    layer.sSliceArgument.uiSliceNum = 1;
    // VUI colour description, matching `yuv::rgba_to_i420`.
    layer.bVideoSignalTypePresent = true;
    layer.uiVideoFormat = 5; // unspecified
    layer.bFullRange = false;
    layer.bColorDescriptionPresent = true;
    layer.uiColorPrimaries = crate::COLOUR_PRIMARIES as u8;
    layer.uiTransferCharacteristics = crate::TRANSFER_CHARACTERISTICS as u8;
    layer.uiColorMatrix = crate::MATRIX_COEFFICIENTS as u8;
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `enc` came from WelsCreateSVCEncoder and is destroyed exactly once.
        unsafe {
            (self.vtbl.Uninitialize.unwrap())(self.enc);
            sys::source::APILoader::WelsDestroySVCEncoder(self.enc);
        }
    }
}
