//! CPU-side image decode (cold path only). Pixels are converted to linear float
//! RGBA and uploaded once to the GPU; develop never re-reads host pixels.
//!
//! RAW loading is progressive (similar to Lightroom):
//! 1. **Embedded JPEG thumbnail** — near-instant, often 1–3 MP
//! 2. **Half-size demosaic** — LibRaw `half_size` + 8-bit process (~4× fewer pixels)
//! 3. **Full-res** — only for export / optional high-quality path

use std::path::{Path, PathBuf};
use std::time::Instant;

/// How thoroughly the pixels were decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeQuality {
    /// Embedded camera preview JPEG (not demosaiced RAW).
    Thumbnail,
    /// LibRaw half-size demosaic (good for interactive develop).
    HalfSize,
    /// Full-resolution demosaic (export / 1:1).
    Full,
}

impl DecodeQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Thumbnail => "thumbnail",
            Self::HalfSize => "half-size",
            Self::Full => "full-res",
        }
    }
}

/// Decoded working image in **linear** float RGBA (not sRGB).
#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// Length = width * height * 4, row-major RGBA.
    pub rgba_f32: Vec<f32>,
    pub label: String,
    pub quality: DecodeQuality,
    /// Original path (kept for future sidecar / catalog use).
    #[allow(dead_code)]
    pub source_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ImageIoError {
    Io(std::io::Error),
    Decode(String),
    Unsupported(String),
}

impl std::fmt::Display for ImageIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Decode(e) => write!(f, "Decode error: {e}"),
            Self::Unsupported(e) => write!(f, "Unsupported: {e}"),
        }
    }
}

impl std::error::Error for ImageIoError {}

impl From<std::io::Error> for ImageIoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Extensions treated as camera RAW (LibRaw via rsraw).
const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "nrw", "arw", "raf", "rw2", "orf", "pef", "dng", "rwl", "iiq", "3fr",
    "fff", "raw", "rwz",
];

pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| RAW_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string()
}

/// Load JPEG/PNG/etc. (full quality in one shot).
pub fn load_image(path: &Path) -> Result<DecodedImage, ImageIoError> {
    let label = file_label(path);
    if is_raw_path(path) {
        // Prefer interactive quality for a single-shot open
        load_raw(path, label, DecodeQuality::HalfSize)
    } else {
        load_raster(path, label)
    }
}

/// Progressive RAW stages: thumbnail → half-size → (optional) full.
///
/// Call from a background thread; each successful stage should be shown ASAP.
pub fn load_raw_progressive(
    path: &Path,
    mut on_stage: impl FnMut(DecodedImage),
) -> Result<(), ImageIoError> {
    let t0 = Instant::now();
    let label = file_label(path);

    // Stage 1: embedded preview (often tens of ms)
    match load_raw_thumbnail(path, &label) {
        Ok(img) => {
            log::info!(
                "RAW stage thumbnail: {}×{} in {:.0} ms",
                img.width,
                img.height,
                t0.elapsed().as_secs_f32() * 1000.0
            );
            on_stage(img);
        }
        Err(e) => {
            log::debug!("RAW thumbnail skipped: {e}");
        }
    }

    // Stage 2: half-size demosaic (interactive develop quality)
    let t1 = Instant::now();
    let half = load_raw(path, label.clone(), DecodeQuality::HalfSize)?;
    log::info!(
        "RAW stage half-size: {}×{} in {:.0} ms (total {:.0} ms)",
        half.width,
        half.height,
        t1.elapsed().as_secs_f32() * 1000.0,
        t0.elapsed().as_secs_f32() * 1000.0
    );
    on_stage(half);

    Ok(())
}

/// Full-resolution demosaic for export.
pub fn load_raw_full(path: &Path) -> Result<DecodedImage, ImageIoError> {
    let t0 = Instant::now();
    let img = load_raw(path, file_label(path), DecodeQuality::Full)?;
    log::info!(
        "RAW full-res: {}×{} in {:.0} ms",
        img.width,
        img.height,
        t0.elapsed().as_secs_f32() * 1000.0
    );
    Ok(img)
}

fn load_raster(path: &Path, label: String) -> Result<DecodedImage, ImageIoError> {
    let img = image::open(path).map_err(|e| ImageIoError::Decode(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgba_f32 = Vec::with_capacity((width * height * 4) as usize);
    for px in rgba.pixels() {
        let r = srgb_to_linear(px[0] as f32 / 255.0);
        let g = srgb_to_linear(px[1] as f32 / 255.0);
        let b = srgb_to_linear(px[2] as f32 / 255.0);
        let a = px[3] as f32 / 255.0;
        rgba_f32.extend_from_slice(&[r, g, b, a]);
    }
    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality: DecodeQuality::Full,
        source_path: Some(path.to_path_buf()),
    })
}

/// Decode the largest usable embedded JPEG/bitmap preview (no demosaic).
#[cfg(feature = "raw")]
fn load_raw_thumbnail(path: &Path, label: &str) -> Result<DecodedImage, ImageIoError> {
    use rsraw::{RawImage, ThumbFormat};

    let data = std::fs::read(path)?;
    let mut raw = RawImage::open(&data).map_err(|e| ImageIoError::Decode(format!("{e:?}")))?;

    let info = raw.full_info();
    let label = if !info.make.is_empty() || !info.model.is_empty() {
        format!("{} — {} {}", label, info.make.trim(), info.model.trim())
    } else {
        label.to_string()
    };

    let thumbs = raw
        .extract_thumbs()
        .map_err(|e| ImageIoError::Decode(format!("thumbs: {e:?}")))?;

    // Prefer largest JPEG; fall back to largest bitmap
    let mut best_jpeg: Option<&rsraw::ThumbnailImage> = None;
    let mut best_bmp: Option<&rsraw::ThumbnailImage> = None;
    for t in &thumbs {
        let area = t.width.saturating_mul(t.height);
        if area < 64 * 64 {
            continue;
        }
        match t.format {
            ThumbFormat::Jpeg => {
                if best_jpeg.map(|b| b.width * b.height).unwrap_or(0) < area {
                    best_jpeg = Some(t);
                }
            }
            ThumbFormat::Bitmap | ThumbFormat::Bitmap16 => {
                if best_bmp.map(|b| b.width * b.height).unwrap_or(0) < area {
                    best_bmp = Some(t);
                }
            }
            _ => {}
        }
    }

    if let Some(t) = best_jpeg {
        let dyn_img =
            image::load_from_memory(&t.data).map_err(|e| ImageIoError::Decode(e.to_string()))?;
        return rgba8_to_decoded(
            &dyn_img.to_rgba8(),
            label,
            DecodeQuality::Thumbnail,
            Some(path.to_path_buf()),
        );
    }

    if let Some(t) = best_bmp {
        return bitmap_thumb_to_decoded(t, label, path);
    }

    Err(ImageIoError::Decode("no usable embedded thumbnail".into()))
}

#[cfg(feature = "raw")]
fn bitmap_thumb_to_decoded(
    t: &rsraw::ThumbnailImage,
    label: String,
    path: &Path,
) -> Result<DecodedImage, ImageIoError> {
    use rsraw::ThumbFormat;

    let w = t.width as usize;
    let h = t.height as usize;
    let colors = t.colors.max(1) as usize;
    let mut rgba_f32 = Vec::with_capacity(w * h * 4);

    match t.format {
        ThumbFormat::Bitmap => {
            let expected = w * h * colors;
            if t.data.len() < expected {
                return Err(ImageIoError::Decode("bitmap thumb too small".into()));
            }
            for i in 0..(w * h) {
                let base = i * colors;
                let r = srgb_to_linear(t.data[base] as f32 / 255.0);
                let g = srgb_to_linear(t.data[base + 1.min(colors - 1)] as f32 / 255.0);
                let b = srgb_to_linear(t.data[base + 2.min(colors - 1)] as f32 / 255.0);
                rgba_f32.extend_from_slice(&[r, g, b, 1.0]);
            }
        }
        ThumbFormat::Bitmap16 => {
            let expected_bytes = w * h * colors * 2;
            if t.data.len() < expected_bytes {
                return Err(ImageIoError::Decode("bitmap16 thumb too small".into()));
            }
            for i in 0..(w * h) {
                let base = i * colors * 2;
                let r16 = u16::from_le_bytes([t.data[base], t.data[base + 1]]);
                let go = base + 2.min((colors - 1) * 2);
                let bo = base + 4.min((colors - 1) * 2);
                let g16 = u16::from_le_bytes([t.data[go], t.data[go + 1]]);
                let b16 = u16::from_le_bytes([t.data[bo], t.data[bo + 1]]);
                rgba_f32.extend_from_slice(&[
                    srgb_to_linear(r16 as f32 / 65535.0),
                    srgb_to_linear(g16 as f32 / 65535.0),
                    srgb_to_linear(b16 as f32 / 65535.0),
                    1.0,
                ]);
            }
        }
        _ => {
            return Err(ImageIoError::Unsupported("thumb format".into()));
        }
    }

    Ok(DecodedImage {
        width: t.width,
        height: t.height,
        rgba_f32,
        label,
        quality: DecodeQuality::Thumbnail,
        source_path: Some(path.to_path_buf()),
    })
}

fn rgba8_to_decoded(
    rgba: &image::RgbaImage,
    label: String,
    quality: DecodeQuality,
    source_path: Option<PathBuf>,
) -> Result<DecodedImage, ImageIoError> {
    let (width, height) = rgba.dimensions();
    let mut rgba_f32 = Vec::with_capacity((width * height * 4) as usize);
    for px in rgba.pixels() {
        rgba_f32.extend_from_slice(&[
            srgb_to_linear(px[0] as f32 / 255.0),
            srgb_to_linear(px[1] as f32 / 255.0),
            srgb_to_linear(px[2] as f32 / 255.0),
            px[3] as f32 / 255.0,
        ]);
    }
    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality,
        source_path,
    })
}

#[cfg(feature = "raw")]
fn load_raw(
    path: &Path,
    label: String,
    quality: DecodeQuality,
) -> Result<DecodedImage, ImageIoError> {
    use rsraw::{RawImage, BIT_DEPTH_16, BIT_DEPTH_8};

    let data = std::fs::read(path)?;
    let mut raw = RawImage::open(&data).map_err(|e| ImageIoError::Decode(format!("{e:?}")))?;

    // Prefer camera white balance before demosaic/process
    raw.set_use_camera_wb(true);
    raw.set_use_camera_matrix(true);

    // Half-size must be set BEFORE unpack — ~4× fewer pixels for demosaic
    let half = quality == DecodeQuality::HalfSize;
    if half {
        raw.set_half_size(true);
    }

    let info = raw.full_info();
    let label = if !info.make.is_empty() || !info.model.is_empty() {
        format!("{} — {} {}", label, info.make.trim(), info.model.trim())
    } else {
        label
    };

    raw.unpack()
        .map_err(|e| ImageIoError::Decode(format!("unpack: {e:?}")))?;

    // 8-bit is enough for interactive preview; 16-bit for full export
    let use_16 = matches!(quality, DecodeQuality::Full);

    if use_16 {
        let processed = raw
            .process::<BIT_DEPTH_16>()
            .map_err(|e| ImageIoError::Decode(format!("process: {e:?}")))?;
        processed_u16_to_decoded(
            &processed,
            label,
            if half {
                DecodeQuality::HalfSize
            } else {
                DecodeQuality::Full
            },
            Some(path.to_path_buf()),
        )
    } else {
        let processed = raw
            .process::<BIT_DEPTH_8>()
            .map_err(|e| ImageIoError::Decode(format!("process: {e:?}")))?;
        processed_u8_to_decoded(
            &processed,
            label,
            DecodeQuality::HalfSize,
            Some(path.to_path_buf()),
        )
    }
}

#[cfg(feature = "raw")]
fn processed_u16_to_decoded(
    processed: &rsraw::ProcessedImage<{ rsraw::BIT_DEPTH_16 }>,
    label: String,
    quality: DecodeQuality,
    source_path: Option<PathBuf>,
) -> Result<DecodedImage, ImageIoError> {
    let width = processed.width();
    let height = processed.height();
    let colors = processed.colors() as usize;
    let samples: &[u16] = processed;
    let spp = colors.max(3);
    let pixels = width as usize * height as usize;
    if samples.len() < pixels * spp {
        return Err(ImageIoError::Decode("raw buffer too small".into()));
    }

    let mut rgba_f32 = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        let base = i * spp;
        let r = srgb_to_linear(samples[base] as f32 / 65535.0);
        let g = srgb_to_linear(samples[base + 1] as f32 / 65535.0);
        let b = srgb_to_linear(
            if colors >= 3 {
                samples[base + 2]
            } else {
                samples[base + 1]
            } as f32
                / 65535.0,
        );
        rgba_f32.extend_from_slice(&[r, g, b, 1.0]);
    }

    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality,
        source_path,
    })
}

#[cfg(feature = "raw")]
fn processed_u8_to_decoded(
    processed: &rsraw::ProcessedImage<{ rsraw::BIT_DEPTH_8 }>,
    label: String,
    quality: DecodeQuality,
    source_path: Option<PathBuf>,
) -> Result<DecodedImage, ImageIoError> {
    let width = processed.width();
    let height = processed.height();
    let colors = processed.colors() as usize;
    let samples: &[u8] = processed;
    let spp = colors.max(3);
    let pixels = width as usize * height as usize;
    if samples.len() < pixels * spp {
        return Err(ImageIoError::Decode("raw buffer too small".into()));
    }

    let mut rgba_f32 = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        let base = i * spp;
        let r = srgb_to_linear(samples[base] as f32 / 255.0);
        let g = srgb_to_linear(samples[base + 1] as f32 / 255.0);
        let b = srgb_to_linear(
            if colors >= 3 {
                samples[base + 2]
            } else {
                samples[base + 1]
            } as f32
                / 255.0,
        );
        rgba_f32.extend_from_slice(&[r, g, b, 1.0]);
    }

    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality,
        source_path,
    })
}

#[cfg(not(feature = "raw"))]
fn load_raw_thumbnail(_path: &Path, _label: &str) -> Result<DecodedImage, ImageIoError> {
    Err(ImageIoError::Unsupported("raw feature disabled".into()))
}

#[cfg(not(feature = "raw"))]
fn load_raw(
    path: &Path,
    _label: String,
    _quality: DecodeQuality,
) -> Result<DecodedImage, ImageIoError> {
    Err(ImageIoError::Unsupported(format!(
        "RAW support is disabled (file {}). Rebuild with default features or `--features raw` \
         (set LIBCLANG_PATH to your LLVM bin folder on Windows). See README.",
        path.display()
    )))
}

/// Encode linear RGBA f32 to sRGB PNG/JPEG bytes for export.
pub fn save_srgb_image(
    path: &Path,
    width: u32,
    height: u32,
    linear_rgba: &[f32],
) -> Result<(), ImageIoError> {
    fn linear_to_srgb(c: f32) -> u8 {
        let c = c.clamp(0.0, 1.0);
        let s = if c <= 0.0031308 {
            12.92 * c
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    }

    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for chunk in linear_rgba.chunks_exact(4) {
        bytes.push(linear_to_srgb(chunk[0]));
        bytes.push(linear_to_srgb(chunk[1]));
        bytes.push(linear_to_srgb(chunk[2]));
        bytes.push((chunk[3].clamp(0.0, 1.0) * 255.0).round() as u8);
    }

    let img = image::RgbaImage::from_raw(width, height, bytes)
        .ok_or_else(|| ImageIoError::Decode("failed to build export image buffer".into()))?;

    img.save(path)
        .map_err(|e| ImageIoError::Decode(e.to_string()))
}
