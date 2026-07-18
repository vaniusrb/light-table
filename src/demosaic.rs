//! Phase 4b/4c — Bayer mosaic demosaic.
//!
//! - **Mode 0 (half):** 2×2 bin → one RGB (fast develop proxy for huge sensors)
//! - **Mode 1 (full bilinear):** classic bilinear CFA interpolate at full active size
//!
//! CPU twins of the GPU `fs_demosaic` shader. Export full-res still uses LibRaw process
//! (higher-quality demosaic); these paths drive interactive develop + export proxy.

use bytemuck::{Pod, Zeroable};

/// Demosaic quality / resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum DemosaicMode {
    /// 2×2 cell → one pixel (output ≈ mosaic/2).
    Half = 0,
    /// Full-resolution bilinear (output = mosaic size).
    FullBilinear = 1,
}

impl DemosaicMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Half => "half 2×2",
            Self::FullBilinear => "full bilinear",
        }
    }
}

/// Uniforms for GPU demosaic — must match shader `DemosaicGpuParams` exactly.
/// Flat fields only (SPIR-V uniform buffer layout rejects tight `f32` arrays).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DemosaicGpuParams {
    pub black: f32,
    /// `1 / max(maximum - black, 1)`.
    pub inv_range: f32,
    pub filters: u32,
    /// [`DemosaicMode`] as u32.
    pub mode: u32,
    pub cam_mul_r: f32,
    pub cam_mul_g: f32,
    pub cam_mul_b: f32,
    pub cam_mul_g2: f32,
    pub m00: f32,
    pub m01: f32,
    pub m02: f32,
    pub _pad0: f32,
    pub m10: f32,
    pub m11: f32,
    pub m12: f32,
    pub _pad1: f32,
    pub m20: f32,
    pub m21: f32,
    pub m22: f32,
    pub _pad2: f32,
    pub mosaic_w: f32,
    pub mosaic_h: f32,
    pub out_w: f32,
    pub out_h: f32,
}

/// Unpacked Bayer mosaic (active area) + color metadata for demosaic.
#[derive(Clone, Debug)]
pub struct MosaicBuffer {
    pub width: u32,
    pub height: u32,
    /// Tight row-major active-area samples (`width * height`).
    pub samples: Vec<u16>,
    pub black: u32,
    pub maximum: u32,
    pub cam_mul: [f32; 4],
    pub rgb_cam: [[f32; 3]; 3],
    pub filters: u32,
    /// Camera orientation (LibRaw `sizes.flip`) — apply via crop, not pixel rotate.
    pub orientation: crate::orient::ImageOrientation,
    pub label: String,
    pub source_path: Option<std::path::PathBuf>,
}

impl MosaicBuffer {
    pub fn is_bayer(&self) -> bool {
        self.filters != 0 && self.filters != !0
    }

    /// Half-size output dimensions (2×2 bin).
    pub fn half_dims(&self) -> (u32, u32) {
        ((self.width / 2).max(1), (self.height / 2).max(1))
    }

    /// Output size for a demosaic mode.
    pub fn out_dims(&self, mode: DemosaicMode) -> (u32, u32) {
        match mode {
            DemosaicMode::Half => self.half_dims(),
            DemosaicMode::FullBilinear => (self.width.max(1), self.height.max(1)),
        }
    }

    /// Prefer full bilinear when the result fits the interactive GPU edge budget.
    pub fn select_mode(&self, max_edge: u32) -> DemosaicMode {
        let max_edge = max_edge.max(64);
        if self.width.max(self.height) <= max_edge {
            DemosaicMode::FullBilinear
        } else {
            DemosaicMode::Half
        }
    }

    pub fn gpu_params(&self, mode: DemosaicMode) -> DemosaicGpuParams {
        let black = self.black as f32;
        let maxv = self.maximum.max(self.black + 1) as f32;
        let inv_range = 1.0 / (maxv - black);

        // Normalize cam_mul so green ≈ 1 (LibRaw-style).
        let g = self.cam_mul[1].max(self.cam_mul[3]).max(1e-6);
        let cam_mul = [
            self.cam_mul[0] / g,
            self.cam_mul[1] / g,
            self.cam_mul[2] / g,
            self.cam_mul[3] / g,
        ];

        let m = self.rgb_cam;
        let (out_w, out_h) = self.out_dims(mode);

        DemosaicGpuParams {
            black,
            inv_range,
            filters: self.filters,
            mode: mode as u32,
            cam_mul_r: cam_mul[0],
            cam_mul_g: cam_mul[1],
            cam_mul_b: cam_mul[2],
            cam_mul_g2: cam_mul[3],
            m00: m[0][0],
            m01: m[0][1],
            m02: m[0][2],
            _pad0: 0.0,
            m10: m[1][0],
            m11: m[1][1],
            m12: m[1][2],
            _pad1: 0.0,
            m20: m[2][0],
            m21: m[2][1],
            m22: m[2][2],
            _pad2: 0.0,
            mosaic_w: self.width as f32,
            mosaic_h: self.height as f32,
            out_w: out_w as f32,
            out_h: out_h as f32,
        }
    }

}

/// LibRaw `FC(row,col)` — CFA color index 0=R, 1=G, 2=B, 3=G2.
#[inline]
pub fn cfa_color(filters: u32, row: u32, col: u32) -> u32 {
    (filters >> ((((row << 1) & 14) + (col & 1)) << 1)) & 3
}

#[inline]
fn is_red(c: u32) -> bool {
    c == 0
}

#[inline]
fn is_blue(c: u32) -> bool {
    c == 2
}

#[inline]
fn is_green(c: u32) -> bool {
    c == 1 || c == 3
}

/// Linearize one mosaic sample: black subtract + scale to ~0..1.
#[inline]
fn linearize(sample: u16, black: f32, inv_range: f32) -> f32 {
    ((sample as f32 - black) * inv_range).max(0.0)
}

/// Soften WB multipliers near sensor clip (must match GPU `soft_wb_mul`).
///
/// At full saturation every CFA site is ~1.0; applying full `cam_mul` then makes
/// R/B >> G → **magenta clipped whites**. Pull multipliers toward 1 as v→1.
#[inline]
fn soft_wb_mul(v: f32, mul: f32) -> f32 {
    const T0: f32 = 0.82;
    if v <= T0 {
        mul
    } else {
        let t = ((v - T0) / (1.0 - T0)).clamp(0.0, 1.0);
        // smoothstep
        let t = t * t * (3.0 - 2.0 * t);
        mul + (1.0 - mul) * t
    }
}

/// White balance with highlight recovery (CPU twin of GPU path).
#[inline]
fn wb_with_highlight_recovery(r: f32, g: f32, b: f32, p: &DemosaicGpuParams) -> (f32, f32, f32) {
    let r0 = r.max(0.0);
    let g0 = g.max(0.0);
    let b0 = b.max(0.0);

    let mut r1 = r0 * soft_wb_mul(r0, p.cam_mul_r);
    let mut g1 = g0 * soft_wb_mul(g0, p.cam_mul_g);
    let mut b1 = b0 * soft_wb_mul(b0, p.cam_mul_b);

    // Partial clip / demosaic mix: if the pixel is in highlight but channels still
    // diverge, blend toward neutral at the brightest post-WB channel.
    const T0: f32 = 0.82;
    let pre_max = r0.max(g0).max(b0);
    if pre_max > T0 {
        let t = ((pre_max - T0) / (1.0 - T0)).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        let hi = r1.max(g1).max(b1);
        r1 += (hi - r1) * t;
        g1 += (hi - g1) * t;
        b1 += (hi - b1) * t;
    }

    (r1, g1, b1)
}

/// Apply white balance (highlight-safe) + camera RGB → linear sRGB matrix.
#[inline]
fn cam_to_linear_srgb(r: f32, g: f32, b: f32, p: &DemosaicGpuParams) -> [f32; 3] {
    let (r, g, b) = wb_with_highlight_recovery(r, g, b, p);
    let rr = p.m00 * r + p.m01 * g + p.m02 * b;
    let gg = p.m10 * r + p.m11 * g + p.m12 * b;
    let bb = p.m20 * r + p.m21 * g + p.m22 * b;
    [rr.max(0.0), gg.max(0.0), bb.max(0.0)]
}

#[inline]
fn clamp_i(v: i32, max: i32) -> i32 {
    if v < 0 {
        0
    } else if v > max {
        max
    } else {
        v
    }
}

#[inline]
fn sample_lin(mosaic: &MosaicBuffer, col: i32, row: i32, p: &DemosaicGpuParams) -> f32 {
    let mw = mosaic.width as i32;
    let mh = mosaic.height as i32;
    let c = clamp_i(col, mw - 1) as usize;
    let r = clamp_i(row, mh - 1) as usize;
    let s = mosaic.samples[r * mosaic.width as usize + c];
    linearize(s, p.black, p.inv_range)
}

/// Classic bilinear: recover R,G,B at one mosaic site.
fn bilinear_rgb_at(
    mosaic: &MosaicBuffer,
    col: u32,
    row: u32,
    p: &DemosaicGpuParams,
) -> (f32, f32, f32) {
    let x = col as i32;
    let y = row as i32;
    let filters = p.filters;
    let c = cfa_color(filters, row, col);
    let v = sample_lin(mosaic, x, y, p);

    // --- Green ---
    let g = if is_green(c) {
        v
    } else {
        // Average orthogonal greens
        let n = sample_lin(mosaic, x, y - 1, p);
        let s = sample_lin(mosaic, x, y + 1, p);
        let e = sample_lin(mosaic, x + 1, y, p);
        let w = sample_lin(mosaic, x - 1, y, p);
        (n + s + e + w) * 0.25
    };

    // --- Red ---
    let r = if is_red(c) {
        v
    } else if is_blue(c) {
        // Diagonals are R on a standard Bayer
        let nw = sample_lin(mosaic, x - 1, y - 1, p);
        let ne = sample_lin(mosaic, x + 1, y - 1, p);
        let sw = sample_lin(mosaic, x - 1, y + 1, p);
        let se = sample_lin(mosaic, x + 1, y + 1, p);
        (nw + ne + sw + se) * 0.25
    } else {
        // Green site: R is either horizontal or vertical neighbor
        let right = cfa_color(filters, row, col.wrapping_add(1));
        if is_red(right) || is_red(cfa_color(filters, row, col.wrapping_sub(1))) {
            let e = sample_lin(mosaic, x + 1, y, p);
            let w = sample_lin(mosaic, x - 1, y, p);
            (e + w) * 0.5
        } else {
            let n = sample_lin(mosaic, x, y - 1, p);
            let s = sample_lin(mosaic, x, y + 1, p);
            (n + s) * 0.5
        }
    };

    // --- Blue ---
    let b = if is_blue(c) {
        v
    } else if is_red(c) {
        let nw = sample_lin(mosaic, x - 1, y - 1, p);
        let ne = sample_lin(mosaic, x + 1, y - 1, p);
        let sw = sample_lin(mosaic, x - 1, y + 1, p);
        let se = sample_lin(mosaic, x + 1, y + 1, p);
        (nw + ne + sw + se) * 0.25
    } else {
        let right = cfa_color(filters, row, col.wrapping_add(1));
        if is_blue(right) || is_blue(cfa_color(filters, row, col.wrapping_sub(1))) {
            let e = sample_lin(mosaic, x + 1, y, p);
            let w = sample_lin(mosaic, x - 1, y, p);
            (e + w) * 0.5
        } else {
            let n = sample_lin(mosaic, x, y - 1, p);
            let s = sample_lin(mosaic, x, y + 1, p);
            (n + s) * 0.5
        }
    };

    (r, g, b)
}

/// Run demosaic for the given mode → (width, height, RGBA linear sRGB).
pub fn demosaic_bayer(mosaic: &MosaicBuffer, mode: DemosaicMode) -> (u32, u32, Vec<f32>) {
    match mode {
        DemosaicMode::Half => demosaic_bayer_half(mosaic),
        DemosaicMode::FullBilinear => demosaic_bayer_full(mosaic),
    }
}

/// Half-size Bayer demosaic (2×2 cells → one RGB). CPU twin of GPU mode 0.
pub fn demosaic_bayer_half(mosaic: &MosaicBuffer) -> (u32, u32, Vec<f32>) {
    let p = mosaic.gpu_params(DemosaicMode::Half);
    let mw = mosaic.width as usize;
    let mh = mosaic.height as usize;
    let (ow, oh) = mosaic.half_dims();
    let mut rgba = Vec::with_capacity((ow * oh * 4) as usize);

    for oy in 0..oh as usize {
        for ox in 0..ow as usize {
            let r0 = (oy * 2) as u32;
            let c0 = (ox * 2) as u32;
            let mut sum_r = 0.0f32;
            let mut sum_g = 0.0f32;
            let mut sum_b = 0.0f32;
            let mut n_r = 0u32;
            let mut n_g = 0u32;
            let mut n_b = 0u32;

            for dy in 0..2u32 {
                for dx in 0..2u32 {
                    let row = r0 + dy;
                    let col = c0 + dx;
                    if row as usize >= mh || col as usize >= mw {
                        continue;
                    }
                    let s = mosaic.samples[row as usize * mw + col as usize];
                    let v = linearize(s, p.black, p.inv_range);
                    match cfa_color(p.filters, row, col) {
                        0 => {
                            sum_r += v;
                            n_r += 1;
                        }
                        2 => {
                            sum_b += v;
                            n_b += 1;
                        }
                        _ => {
                            sum_g += v;
                            n_g += 1;
                        }
                    }
                }
            }

            let r = if n_r > 0 { sum_r / n_r as f32 } else { 0.0 };
            let g = if n_g > 0 { sum_g / n_g as f32 } else { 0.0 };
            let b = if n_b > 0 { sum_b / n_b as f32 } else { 0.0 };
            let [lr, lg, lb] = cam_to_linear_srgb(r, g, b, &p);
            rgba.extend_from_slice(&[lr, lg, lb, 1.0]);
        }
    }

    (ow, oh, rgba)
}

/// Full-resolution bilinear Bayer demosaic. CPU twin of GPU mode 1.
pub fn demosaic_bayer_full(mosaic: &MosaicBuffer) -> (u32, u32, Vec<f32>) {
    let p = mosaic.gpu_params(DemosaicMode::FullBilinear);
    let ow = mosaic.width;
    let oh = mosaic.height;
    let mut rgba = Vec::with_capacity((ow * oh * 4) as usize);

    for row in 0..oh {
        for col in 0..ow {
            let (r, g, b) = bilinear_rgb_at(mosaic, col, row, &p);
            let [lr, lg, lb] = cam_to_linear_srgb(r, g, b, &p);
            rgba.extend_from_slice(&[lr, lg, lb, 1.0]);
        }
    }

    (ow, oh, rgba)
}
