//! RGBA8 -> planar YUV 4:2:0 (I420), BT.709 limited ("TV") range.
//!
//! Fixed-point (16 fractional bits), integer-only inner loop, no allocation.
//! Chroma is computed from the 2x2 average of the RGB values, which equals the
//! average of the per-pixel chroma because the transform is linear.

/// BT.709 luma coefficients.
const KR: f64 = 0.2126;
const KB: f64 = 0.0722;
const KG: f64 = 1.0 - KR - KB;

const SHIFT: u32 = 16;
const ONE: f64 = (1u32 << SHIFT) as f64;
/// Limited range: luma spans 219 codes (16..=235), chroma 224 codes (16..=240).
const Y_SCALE: f64 = 219.0 / 255.0 * ONE;
const C_SCALE: f64 = 224.0 / 255.0 * ONE;

const fn fix(x: f64) -> i32 {
    // Round half away from zero (const-friendly; f64::round is not const).
    if x >= 0.0 {
        (x + 0.5) as i32
    } else {
        (x - 0.5) as i32
    }
}

const Y_R: i32 = fix(KR * Y_SCALE);
const Y_G: i32 = fix(KG * Y_SCALE);
const Y_B: i32 = fix(KB * Y_SCALE);
const Y_OFF: i32 = (16 << SHIFT) + (1 << (SHIFT - 1));

// Cb = (B - Y) / (2 (1 - KB)),  Cr = (R - Y) / (2 (1 - KR)), scaled to 224 codes.
const U_R: i32 = fix(-KR / (2.0 * (1.0 - KB)) * C_SCALE);
const U_G: i32 = fix(-KG / (2.0 * (1.0 - KB)) * C_SCALE);
const U_B: i32 = -(U_R + U_G); // rows of the chroma matrix sum to zero exactly
const V_R: i32 = fix(0.5 * C_SCALE);
const V_B: i32 = fix(-KB / (2.0 * (1.0 - KR)) * C_SCALE);
const V_G: i32 = -(V_R + V_B);
/// Chroma takes the *sum* of 4 pixels, hence the extra 2 bits of shift.
const C_SHIFT: u32 = SHIFT + 2;
const C_OFF: i32 = (128 << C_SHIFT) + (1 << (C_SHIFT - 1));

/// Luma for one row. Kept as a separate simple loop so LLVM vectorizes it.
#[inline(always)]
fn luma_row(src: &[u8], dst: &mut [u8]) {
    for (px, y) in src.chunks_exact(4).zip(dst) {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        *y = ((Y_R * r + Y_G * g + Y_B * b + Y_OFF) >> SHIFT) as u8;
    }
}

/// Chroma for one pair of rows from the 2x2 RGB sums.
#[inline(always)]
fn chroma_row(src0: &[u8], src1: &[u8], dst_u: &mut [u8], dst_v: &mut [u8]) {
    let blocks = src0.chunks_exact(8).zip(src1.chunks_exact(8));
    for ((a, b), (u, v)) in blocks.zip(dst_u.iter_mut().zip(dst_v)) {
        let sum = |c: usize| a[c] as i32 + a[c + 4] as i32 + b[c] as i32 + b[c + 4] as i32;
        let (r, g, bl) = (sum(0), sum(1), sum(2));
        *u = ((U_R * r + U_G * g + U_B * bl + C_OFF) >> C_SHIFT) as u8;
        *v = ((V_R * r + V_G * g + V_B * bl + C_OFF) >> C_SHIFT) as u8;
    }
}

/// Converts one RGBA frame into the three I420 planes. Alpha is ignored.
///
/// `width` and `height` must be even; plane lengths must be `w*h`, `w*h/4`, `w*h/4`.
pub fn rgba_to_i420(rgba: &[u8], width: usize, height: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    // The default x86-64 target only has SSE2, which lacks 32-bit vector
    // multiplies; an AVX2 build of the same loops is ~4x faster.
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: AVX2 support was just checked.
        return unsafe { convert_avx2(rgba, width, height, y, u, v) };
    }
    convert(rgba, width, height, y, u, v)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn convert_avx2(rgba: &[u8], width: usize, height: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    convert(rgba, width, height, y, u, v)
}

#[inline(always)]
fn convert(rgba: &[u8], width: usize, height: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let cw = width / 2;
    assert!(width.is_multiple_of(2) && height.is_multiple_of(2));
    assert_eq!(rgba.len(), width * height * 4);
    assert_eq!(y.len(), width * height);
    assert!(u.len() == cw * height / 2 && v.len() == u.len());

    let rows = rgba.chunks_exact(width * 8); // two RGBA rows at a time
    let y_rows = y.chunks_exact_mut(width * 2);
    let uv_rows = u.chunks_exact_mut(cw).zip(v.chunks_exact_mut(cw));
    for ((src, dst_y), (dst_u, dst_v)) in rows.zip(y_rows).zip(uv_rows) {
        let (src0, src1) = src.split_at(width * 4);
        let (y0, y1) = dst_y.split_at_mut(width);
        luma_row(src0, y0);
        luma_row(src1, y1);
        chroma_row(src0, src1, dst_u, dst_v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert_solid(rgb: [u8; 3]) -> (u8, u8, u8) {
        let px = [rgb[0], rgb[1], rgb[2], 255];
        let rgba: Vec<u8> = px.iter().copied().cycle().take(4 * 4 * 4).collect();
        let (mut y, mut u, mut v) = (vec![0; 16], vec![0; 4], vec![0; 4]);
        rgba_to_i420(&rgba, 4, 4, &mut y, &mut u, &mut v);
        assert!(y.iter().all(|&s| s == y[0]) && u.iter().all(|&s| s == u[0]));
        (y[0], u[0], v[0])
    }

    #[test]
    fn reference_colours_bt709_limited() {
        assert_eq!(convert_solid([0, 0, 0]), (16, 128, 128));
        assert_eq!(convert_solid([255, 255, 255]), (235, 128, 128));
        assert_eq!(convert_solid([128, 128, 128]), (126, 128, 128));
        // Values per ITU-R BT.709 limited range (as produced by e.g. ffmpeg/zscale).
        assert_eq!(convert_solid([255, 0, 0]), (63, 102, 240));
        assert_eq!(convert_solid([0, 255, 0]), (173, 42, 26));
        assert_eq!(convert_solid([0, 0, 255]), (32, 240, 118));
    }
}

#[cfg(test)]
mod perf {
    #[test]
    #[ignore]
    fn conv_speed() {
        let (w, h) = (1080, 720);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let (mut y, mut u, mut v) = (vec![0; w * h], vec![0; w * h / 4], vec![0; w * h / 4]);
        let t = std::time::Instant::now();
        for _ in 0..300 {
            super::rgba_to_i420(std::hint::black_box(&rgba), w, h, &mut y, &mut u, &mut v);
        }
        eprintln!("PERF {:.3} ms/frame", t.elapsed().as_secs_f64() / 300.0 * 1000.0);
    }
}
