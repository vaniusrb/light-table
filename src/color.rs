//! Working-color-space policy for light-table.
//!
//! Phase 2 ([`docs/RAW_LINEAR_PIPELINE_PLAN.md`](../docs/RAW_LINEAR_PIPELINE_PLAN.md)):
//! every decoded pixel buffer is **linear RGB in a named space**. The GPU develop
//! path and export assume that space when applying the IEC sRGB *transfer* for display.
//!
//! ## Current app policy (Option A)
//!
//! | Source | Working space | How we get there |
//! |---|---|---|
//! | JPEG / PNG / TIFF / … | [`WorkingSpace::LinearSrgb`] | IEC sRGB inverse EOTF on load |
//! | RAW demosaic (LibRaw) | [`WorkingSpace::LinearSrgb`] | LibRaw linear XYZ → app matrix (Phase 4a) |
//! | RAW mosaic (Bayer GPU) | [`WorkingSpace::LinearSrgb`] | Unpack mosaic → GPU half-Bayer + `rgb_cam` (Phase 4b) |
//! | RAW embedded JPEG thumb | [`WorkingSpace::LinearSrgb`] (via display) | Same inverse EOTF as raster; quality is still “thumbnail” |
//!
//! Display / histogram / export: **linear sRGB → IEC sRGB EOTF** (no extra matrix).
//!
//! Phase 4a: LibRaw **linear XYZ** → [`xyz_to_linear_srgb`] → working linear sRGB.
//! Phase 4b/4c: mosaic + camera matrix → linear sRGB on GPU (Bayer half or full bilinear).

use serde::{Deserialize, Serialize};

/// Named linear RGB space of [`crate::image_io::DecodedImage`] pixels (and the GPU texture).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorkingSpace {
    /// Linear light, **sRGB primaries** (IEC 61966-2-1), D65-relative.
    ///
    /// App-wide working space: GPU develop + export assume this.
    #[default]
    LinearSrgb,
}

impl WorkingSpace {
    pub fn label(self) -> &'static str {
        match self {
            Self::LinearSrgb => "linear sRGB",
        }
    }

    /// True if display/export may use IEC sRGB transfer with **no** RGB matrix.
    pub fn is_linear_srgb_primaries(self) -> bool {
        matches!(self, Self::LinearSrgb)
    }
}

/// How pixels were encoded *before* conversion into [`WorkingSpace`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceEncoding {
    /// 8-bit (or similar) display-referred sRGB file or embedded JPEG thumb.
    DisplaySrgb,
    /// LibRaw demosaic linear in sRGB primaries (`output_color=sRGB`, `gamm=1,1`).
    LibRawLinearSrgb,
    /// LibRaw demosaic linear XYZ → converted to linear sRGB in-app (Phase 4a).
    LibRawLinearXyz,
    /// Bayer mosaic demosaic (GPU half or full bilinear / CPU twin) via black / cam_mul / rgb_cam (4b/4c).
    MosaicBayerLinear,
}

impl SourceEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::DisplaySrgb => "display sRGB → linear",
            Self::LibRawLinearSrgb => "LibRaw linear sRGB",
            Self::LibRawLinearXyz => "LibRaw linear XYZ → sRGB",
            Self::MosaicBayerLinear => "Bayer mosaic → linear sRGB",
        }
    }
}

/// Metadata attached to a decoded buffer (not sent to the GPU as pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorMeta {
    pub working_space: WorkingSpace,
    pub source_encoding: SourceEncoding,
}

impl ColorMeta {
    pub fn from_display_srgb() -> Self {
        Self {
            working_space: WorkingSpace::LinearSrgb,
            source_encoding: SourceEncoding::DisplaySrgb,
        }
    }

    pub fn from_libraw_linear_srgb() -> Self {
        Self {
            working_space: WorkingSpace::LinearSrgb,
            source_encoding: SourceEncoding::LibRawLinearSrgb,
        }
    }

    pub fn from_libraw_linear_xyz() -> Self {
        Self {
            working_space: WorkingSpace::LinearSrgb,
            source_encoding: SourceEncoding::LibRawLinearXyz,
        }
    }

    pub fn from_mosaic_bayer() -> Self {
        Self {
            working_space: WorkingSpace::LinearSrgb,
            source_encoding: SourceEncoding::MosaicBayerLinear,
        }
    }
}

// ── XYZ (D65) → linear sRGB (IEC 61966-2-1 / common D65 matrix) ────────────

/// Convert CIE XYZ (D65, relative) to linear sRGB. Values may be outside 0..1.
#[inline]
pub fn xyz_to_linear_srgb(x: f32, y: f32, z: f32) -> [f32; 3] {
    // OpenGL / IEC-style D65 XYZ→sRGB matrix
    let r = 3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z;
    let g = -0.969_266_0 * x + 1.876_010_8 * y + 0.041_556_0 * z;
    let b = 0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z;
    [r, g, b]
}

/// Normalize LibRaw 16-bit XYZ sample and convert to linear sRGB, soft-clipping negatives.
#[inline]
pub fn libraw_xyz16_to_linear_srgb(x: u16, y: u16, z: u16) -> [f32; 3] {
    // LibRaw scales XYZ into the integer range; treat as relative [0,1] then matrix.
    let xf = x as f32 / 65535.0;
    let yf = y as f32 / 65535.0;
    let zf = z as f32 / 65535.0;
    let [r, g, b] = xyz_to_linear_srgb(xf, yf, zf);
    [r.max(0.0), g.max(0.0), b.max(0.0)]
}

/// Same for 8-bit processed XYZ (unusual but supported).
#[inline]
pub fn libraw_xyz8_to_linear_srgb(x: u8, y: u8, z: u8) -> [f32; 3] {
    let xf = x as f32 / 255.0;
    let yf = y as f32 / 255.0;
    let zf = z as f32 / 255.0;
    let [r, g, b] = xyz_to_linear_srgb(xf, yf, zf);
    [r.max(0.0), g.max(0.0), b.max(0.0)]
}

// ── IEC 61966-2-1 transfer (shared CPU policy; GPU twin in shader) ─────────

/// Display-referred sRGB [0,1] → linear.
#[inline]
pub fn srgb_eotf(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear → display-referred sRGB [0,1].
#[inline]
pub fn srgb_oetf(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear sRGB → 8-bit display sRGB (export).
#[inline]
pub fn linear_srgb_to_u8(c: f32) -> u8 {
    (srgb_oetf(c) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Estimate a **base exposure (EV)** so a linear RAW open looks closer to other apps.
///
/// Honest linear decode (no LibRaw auto-bright) leaves midtones dark because scene
/// data rarely fills the sensor white level. Other software applies auto exposure /
/// baseline EV + tone curves. We only set a constant linear gain as EV:
/// map ~99th-percentile luminance toward a display mid/high target.
///
/// Returns EV in about `[-0.5, +3.5]` (0 = no change). Safe no-op for empty buffers.
pub fn estimate_raw_base_exposure_ev(rgba: &[f32], width: u32, height: u32) -> f32 {
    if width == 0 || height == 0 || rgba.len() < 4 {
        return 0.0;
    }

    // Coarse grid — enough for exposure, cheap on multi-MP buffers
    const GRID: u32 = 96;
    let mut samples = Vec::with_capacity((GRID * GRID) as usize);
    for gy in 0..GRID {
        let y = ((gy as u64 * height as u64) / GRID as u64) as u32;
        let y = y.min(height - 1);
        for gx in 0..GRID {
            let x = ((gx as u64 * width as u64) / GRID as u64) as u32;
            let x = x.min(width - 1);
            let i = ((y * width + x) * 4) as usize;
            if i + 2 >= rgba.len() {
                continue;
            }
            let r = rgba[i];
            let g = rgba[i + 1];
            let b = rgba[i + 2];
            // Rec.709 linear luminance
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            if luma.is_finite() && luma > 0.0 {
                samples.push(luma);
            }
        }
    }

    if samples.is_empty() {
        return 1.0; // very dark / empty → modest lift
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // ~99th percentile (highlight-aware; avoids mean skewed by large dark areas)
    let idx = ((samples.len() as f32 - 1.0) * 0.99).round() as usize;
    let p99 = samples[idx.min(samples.len() - 1)];

    // Target: bright-but-not-clipped linear level after gain (before develop)
    const TARGET: f32 = 0.78;
    if p99 < 1e-6 {
        return 2.0;
    }

    let scale = (TARGET / p99).clamp(0.7, 11.3); // ~-0.5 .. +3.5 EV
    let ev = scale.log2();
    // Snap near-zero so neutral JPEGs-like RAWs don't jitter the slider
    if ev.abs() < 0.05 {
        0.0
    } else {
        (ev * 20.0).round() / 20.0 // 0.05 EV steps
    }
}

/// True if this encoding benefits from auto base exposure (linear RAW, no baked bright).
pub fn needs_raw_base_exposure(enc: SourceEncoding) -> bool {
    matches!(
        enc,
        SourceEncoding::LibRawLinearSrgb
            | SourceEncoding::LibRawLinearXyz
            | SourceEncoding::MosaicBayerLinear
    )
}

/// Documented policy string for logs / status.
pub fn policy_summary() -> &'static str {
    "working space = linear sRGB primaries; display = IEC sRGB OETF; \
     RAW = linear demosaic + auto base EV (no LibRaw auto-bright); JPEG = sRGB EOTF on load"
}
