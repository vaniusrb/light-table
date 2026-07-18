//! CPU-side image decode (cold path only). Pixels are converted to linear float
//! RGBA and uploaded once to the GPU; develop never re-reads host pixels.
//!
//! Working-space policy: see [`crate::color`] (Phase 2 — linear sRGB primaries).
//!
//! RAW loading is progressive (similar to Lightroom):
//! 1. **Embedded JPEG thumbnail** — near-instant
//! 2. **Quarter proxy** (huge files) — half demosaic then 2× box downscale
//! 3. **Half-size demosaic** — Bayer GPU mosaic (Phase 4b) or LibRaw linear XYZ
//! 4. **Full-res** — export only (LibRaw process)
//!
//! GPU working textures are also capped by [`MAX_GPU_PROXY_EDGE`] for interactive filters.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::color::{self, ColorMeta};
use crate::demosaic::{self, DemosaicMode, MosaicBuffer};
use crate::orient::ImageOrientation;

/// If half-size demosaic still exceeds this long edge, emit a quarter stage first
/// and/or clamp the GPU proxy (Phase 3).
pub const MAX_GPU_PROXY_EDGE: u32 = 3200;

/// Treat as “huge” when full-frame max edge ≥ this (triggers quarter progressive stage).
const HUGE_RAW_EDGE: u32 = 5000;

/// How thoroughly the pixels were decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeQuality {
    /// Embedded camera preview JPEG (not demosaiced RAW).
    Thumbnail,
    /// ~1/4 linear dimensions of full frame (half demosaic + 2× downscale).
    Quarter,
    /// LibRaw half-size demosaic (good for interactive develop).
    HalfSize,
    /// Full-resolution demosaic (export / 1:1).
    Full,
}

impl DecodeQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Thumbnail => "thumbnail",
            Self::Quarter => "quarter",
            Self::HalfSize => "half-size",
            Self::Full => "full-res",
        }
    }
}

/// Decoded working image in **linear** float RGBA in [`ColorMeta::working_space`].
#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// Length = width * height * 4, row-major RGBA (linear, working space).
    pub rgba_f32: Vec<f32>,
    pub label: String,
    pub quality: DecodeQuality,
    /// Named linear RGB space + how we arrived there.
    pub color: ColorMeta,
    /// Orientation still needed on the canvas (identity if already baked into pixels).
    ///
    /// - **Mosaic / RAW GPU path:** LibRaw flip → apply via crop
    /// - **LibRaw process / EXIF-baked JPEG:** pixels upright → identity
    pub orientation: ImageOrientation,
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

/// One progressive RAW stage delivered to the UI thread.
pub enum ProgressiveStage {
    /// Raster already in linear working space (thumb / LibRaw / CPU demosaic).
    Image(DecodedImage),
    /// Bayer mosaic for GPU half-size demosaic (Phase 4b).
    Mosaic(MosaicBuffer),
}

/// Progressive RAW stages: thumbnail → [quarter] → half-size (GPU-capped).
///
/// Call from a background thread; each successful stage should be shown ASAP.
/// Full-res is **not** loaded here — only on export via [`load_raw_full`].
///
/// Bayer sensors prefer **unpack + mosaic** (Phase 4b GPU demosaic). Non-Bayer
/// (e.g. X-Trans) falls back to LibRaw linear XYZ process (Phase 4a).
pub fn load_raw_progressive(
    path: &Path,
    mut on_stage: impl FnMut(ProgressiveStage),
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
            on_stage(ProgressiveStage::Image(img));
        }
        Err(e) => {
            log::debug!("RAW thumbnail skipped: {e}");
        }
    }

    // Stage 2–3: prefer Bayer mosaic (GPU demosaic on main thread)
    let t1 = Instant::now();
    match load_raw_mosaic(path, label.clone()) {
        Ok(mosaic) if mosaic.is_bayer() && mosaic.width >= 2 && mosaic.height >= 2 => {
            log::info!(
                "RAW stage mosaic: {}×{} filters=0x{:08x} in {:.0} ms (total {:.0} ms)",
                mosaic.width,
                mosaic.height,
                mosaic.filters,
                t1.elapsed().as_secs_f32() * 1000.0,
                t0.elapsed().as_secs_f32() * 1000.0
            );

            // Optional quarter preview from cheap CPU half-Bayer (huge sensors only)
            let mode = mosaic.select_mode(MAX_GPU_PROXY_EDGE);
            let (ow, oh) = mosaic.out_dims(mode);
            let est_full_edge = mosaic.width.max(mosaic.height);
            let huge = est_full_edge >= HUGE_RAW_EDGE || ow.max(oh) > MAX_GPU_PROXY_EDGE;
            if huge {
                let tq = Instant::now();
                let half_img = mosaic_to_decoded(&mosaic, DemosaicMode::Half);
                let quarter = downsample_box2(&half_img, DecodeQuality::Quarter);
                log::info!(
                    "RAW stage quarter proxy (CPU Bayer): {}×{} in {:.0} ms",
                    quarter.width,
                    quarter.height,
                    tq.elapsed().as_secs_f32() * 1000.0
                );
                on_stage(ProgressiveStage::Image(quarter));
            }

            log::info!(
                "RAW stage develop proxy: mosaic → GPU {} ({}×{} out)",
                mode.label(),
                ow,
                oh
            );
            on_stage(ProgressiveStage::Mosaic(mosaic));
            return Ok(());
        }
        Ok(_) => {
            log::info!("RAW mosaic not Bayer — falling back to LibRaw process");
        }
        Err(e) => {
            log::info!("RAW mosaic path skipped ({e}) — LibRaw process fallback");
        }
    }

    // Fallback: LibRaw linear XYZ half-size demosaic (Phase 4a)
    let half = load_raw(path, label, DecodeQuality::HalfSize)?;
    log::info!(
        "RAW stage half-size LibRaw: {}×{} in {:.0} ms (total {:.0} ms)",
        half.width,
        half.height,
        t1.elapsed().as_secs_f32() * 1000.0,
        t0.elapsed().as_secs_f32() * 1000.0
    );

    let est_full_edge = half.width.max(half.height).saturating_mul(2);
    let huge = est_full_edge >= HUGE_RAW_EDGE
        || half.width.max(half.height) > MAX_GPU_PROXY_EDGE;

    if huge && half.width >= 4 && half.height >= 4 {
        let tq = Instant::now();
        let quarter = downsample_box2(&half, DecodeQuality::Quarter);
        log::info!(
            "RAW stage quarter proxy: {}×{} in {:.0} ms",
            quarter.width,
            quarter.height,
            tq.elapsed().as_secs_f32() * 1000.0
        );
        on_stage(ProgressiveStage::Image(quarter));
    }

    let proxy = limit_max_edge(half, MAX_GPU_PROXY_EDGE);
    log::info!(
        "RAW stage develop proxy: {}×{} quality={:?}",
        proxy.width,
        proxy.height,
        proxy.quality
    );
    on_stage(ProgressiveStage::Image(proxy));

    Ok(())
}

/// CPU Bayer demosaic → [`DecodedImage`] (export proxy / quarter source).
///
/// Mode is chosen with [`MosaicBuffer::select_mode`] unless overridden.
pub fn mosaic_to_decoded(mosaic: &MosaicBuffer, mode: DemosaicMode) -> DecodedImage {
    let (width, height, rgba_f32) = demosaic::demosaic_bayer(mosaic, mode);
    let quality = match mode {
        DemosaicMode::FullBilinear => DecodeQuality::Full,
        DemosaicMode::Half => DecodeQuality::HalfSize,
    };
    let img = DecodedImage {
        width,
        height,
        rgba_f32,
        label: mosaic.label.clone(),
        quality,
        color: ColorMeta::from_mosaic_bayer(),
        // Sensor buffer is unrotated; orientation applied on the canvas via crop.
        orientation: mosaic.orientation,
        source_path: mosaic.source_path.clone(),
    };
    limit_max_edge(img, MAX_GPU_PROXY_EDGE)
}



/// 2×2 box downsample (linear light). Output size = floor(w/2) × floor(h/2).
pub fn downsample_box2(img: &DecodedImage, quality: DecodeQuality) -> DecodedImage {
    let w = img.width as usize;
    let h = img.height as usize;
    let nw = (w / 2).max(1);
    let nh = (h / 2).max(1);
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            let x0 = x * 2;
            let y0 = y * 2;
            let x1 = (x0 + 1).min(w - 1);
            let y1 = (y0 + 1).min(h - 1);
            let mut acc = [0.0f32; 4];
            for (xx, yy) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                let i = (yy * w + xx) * 4;
                acc[0] += img.rgba_f32[i];
                acc[1] += img.rgba_f32[i + 1];
                acc[2] += img.rgba_f32[i + 2];
                acc[3] += img.rgba_f32[i + 3];
            }
            out.extend_from_slice(&[acc[0] * 0.25, acc[1] * 0.25, acc[2] * 0.25, acc[3] * 0.25]);
        }
    }
    DecodedImage {
        width: nw as u32,
        height: nh as u32,
        rgba_f32: out,
        label: img.label.clone(),
        quality,
        color: img.color,
        orientation: img.orientation,
        source_path: img.source_path.clone(),
    }
}

/// Repeatedly 2× box-downsample until max edge ≤ `max_edge` (linear light).
pub fn limit_max_edge(mut img: DecodedImage, max_edge: u32) -> DecodedImage {
    let max_edge = max_edge.max(64);
    while img.width.max(img.height) > max_edge && img.width >= 2 && img.height >= 2 {
        let q = if img.quality == DecodeQuality::Full {
            DecodeQuality::HalfSize
        } else {
            img.quality
        };
        img = downsample_box2(&img, q);
    }
    img
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
    // Apply EXIF orientation so portrait JPEGs are upright in the pixel buffer.
    let dyn_img = open_image_with_orientation(path)?;
    let rgba = dyn_img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgba_f32 = Vec::with_capacity((width * height * 4) as usize);
    for px in rgba.pixels() {
        // Display sRGB → linear sRGB working space
        let r = color::srgb_eotf(px[0] as f32 / 255.0);
        let g = color::srgb_eotf(px[1] as f32 / 255.0);
        let b = color::srgb_eotf(px[2] as f32 / 255.0);
        let a = px[3] as f32 / 255.0;
        rgba_f32.extend_from_slice(&[r, g, b, a]);
    }
    let decoded = DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality: DecodeQuality::Full,
        color: ColorMeta::from_display_srgb(),
        orientation: ImageOrientation::identity(), // baked into pixels
        source_path: Some(path.to_path_buf()),
    };
    // Huge JPEGs: keep interactive GPU path responsive (export re-reads file as Full)
    Ok(limit_max_edge(decoded, MAX_GPU_PROXY_EDGE))
}

/// Open raster and bake EXIF/TIFF orientation into the pixel buffer.
fn open_image_with_orientation(path: &Path) -> Result<image::DynamicImage, ImageIoError> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| ImageIoError::Decode(e.to_string()))?
        .with_guessed_format()
        .map_err(|e| ImageIoError::Decode(e.to_string()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| ImageIoError::Decode(e.to_string()))?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| ImageIoError::Decode(e.to_string()))?;
    if orientation != image::metadata::Orientation::NoTransforms {
        log::info!("Raster EXIF orientation: {orientation:?} (baked into pixels)");
        img.apply_orientation(orientation);
    }
    Ok(img)
}

/// Decode JPEG/PNG bytes and bake orientation when the decoder reports it.
fn load_image_bytes_oriented(data: &[u8]) -> Result<image::DynamicImage, ImageIoError> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| ImageIoError::Decode(e.to_string()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| ImageIoError::Decode(e.to_string()))?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| ImageIoError::Decode(e.to_string()))?;
    if orientation != image::metadata::Orientation::NoTransforms {
        log::info!("Embedded JPEG orientation: {orientation:?} (baked into pixels)");
        img.apply_orientation(orientation);
    }
    Ok(img)
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
        let dyn_img = load_image_bytes_oriented(&t.data)
            .or_else(|_| {
                image::load_from_memory(&t.data).map_err(|e| ImageIoError::Decode(e.to_string()))
            })?;
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
                let r = color::srgb_eotf(t.data[base] as f32 / 255.0);
                let g = color::srgb_eotf(t.data[base + 1.min(colors - 1)] as f32 / 255.0);
                let b = color::srgb_eotf(t.data[base + 2.min(colors - 1)] as f32 / 255.0);
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
                    color::srgb_eotf(r16 as f32 / 65535.0),
                    color::srgb_eotf(g16 as f32 / 65535.0),
                    color::srgb_eotf(b16 as f32 / 65535.0),
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
        color: ColorMeta::from_display_srgb(),
        orientation: ImageOrientation::identity(),
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
            color::srgb_eotf(px[0] as f32 / 255.0),
            color::srgb_eotf(px[1] as f32 / 255.0),
            color::srgb_eotf(px[2] as f32 / 255.0),
            px[3] as f32 / 255.0,
        ]);
    }
    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality,
        color: ColorMeta::from_display_srgb(),
        orientation: ImageOrientation::identity(),
        source_path,
    })
}

/// Unpack Bayer mosaic only (no LibRaw demosaic). Phase 4b GPU path.
#[cfg(feature = "raw")]
pub fn load_raw_mosaic(path: &Path, label: String) -> Result<MosaicBuffer, ImageIoError> {
    use rsraw::RawImage;

    let data = std::fs::read(path)?;
    let mut raw = RawImage::open(&data).map_err(|e| ImageIoError::Decode(format!("{e:?}")))?;

    let info = raw.full_info();
    let label = if !info.make.is_empty() || !info.model.is_empty() {
        format!("{} — {} {}", label, info.make.trim(), info.model.trim())
    } else {
        label
    };

    // Full-size mosaic; half-size is done by the demosaic 2×2 bin (not LibRaw half_size).
    raw.unpack()
        .map_err(|e| ImageIoError::Decode(format!("unpack: {e:?}")))?;

    let sensor = raw.sensor_meta();
    let orientation = ImageOrientation::from_libraw_flip(sensor.flip);
    log::info!(
        "RAW sensor (mosaic): {}×{} (raw {}×{} margin {},{}), black={}, max={}, filters=0x{:08x}, flip={} ({})",
        sensor.width,
        sensor.height,
        sensor.raw_width,
        sensor.raw_height,
        sensor.left_margin,
        sensor.top_margin,
        sensor.black,
        sensor.maximum,
        sensor.filters,
        sensor.flip,
        orientation.label()
    );

    let samples = raw
        .copy_mosaic_u16()
        .map_err(|e| ImageIoError::Decode(format!("mosaic copy: {e:?}")))?;

    if samples.len() != sensor.width as usize * sensor.height as usize {
        return Err(ImageIoError::Decode(format!(
            "mosaic size mismatch: got {} samples for {}×{}",
            samples.len(),
            sensor.width,
            sensor.height
        )));
    }

    Ok(MosaicBuffer {
        width: sensor.width,
        height: sensor.height,
        samples,
        black: sensor.black,
        maximum: sensor.maximum.max(1),
        cam_mul: sensor.cam_mul,
        rgb_cam: sensor.rgb_cam,
        filters: sensor.filters,
        orientation,
        label,
        source_path: Some(path.to_path_buf()),
    })
}

#[cfg(not(feature = "raw"))]
pub fn load_raw_mosaic(_path: &Path, _label: String) -> Result<MosaicBuffer, ImageIoError> {
    Err(ImageIoError::Unsupported("raw feature disabled".into()))
}

#[cfg(feature = "raw")]
fn load_raw(
    path: &Path,
    label: String,
    quality: DecodeQuality,
) -> Result<DecodedImage, ImageIoError> {
    use rsraw::{OutputColor, RawImage, BIT_DEPTH_16, BIT_DEPTH_8};

    let data = std::fs::read(path)?;
    let mut raw = RawImage::open(&data).map_err(|e| ImageIoError::Decode(format!("{e:?}")))?;

    // Phase 4a: linear XYZ demosaic → in-app matrix → linear sRGB working space
    // (docs/RAW_LINEAR_PIPELINE_PLAN.md). Camera WB/matrix still applied by LibRaw.
    raw.set_use_camera_wb(true);
    raw.set_use_camera_matrix(true);
    raw.set_output_color(OutputColor::Xyz);
    raw.set_gamma(1.0, 1.0);
    raw.set_no_auto_bright(true);
    // Blend clipped highlights (reduces magenta cast on overexposed whites)
    raw.set_highlight(2);

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

    // Sensor metadata (also used by mosaic path)
    let sensor = raw.sensor_meta();
    log::info!(
        "RAW sensor: {}×{} (raw {}×{}), black={}, max={}, filters=0x{:08x}, mosaic={}",
        sensor.width,
        sensor.height,
        sensor.raw_width,
        sensor.raw_height,
        sensor.black,
        sensor.maximum,
        sensor.filters,
        sensor.has_raw_image
    );
    log::debug!(
        "RAW cam_mul=[{:.3},{:.3},{:.3},{:.3}]",
        sensor.cam_mul[0],
        sensor.cam_mul[1],
        sensor.cam_mul[2],
        sensor.cam_mul[3]
    );

    // 8-bit is enough for interactive preview; 16-bit for full export
    let use_16 = matches!(quality, DecodeQuality::Full);
    let q = if half {
        DecodeQuality::HalfSize
    } else {
        DecodeQuality::Full
    };

    if use_16 {
        let processed = raw
            .process::<BIT_DEPTH_16>()
            .map_err(|e| ImageIoError::Decode(format!("process: {e:?}")))?;
        processed_xyz16_to_decoded(&processed, label, q, Some(path.to_path_buf()))
    } else {
        let processed = raw
            .process::<BIT_DEPTH_8>()
            .map_err(|e| ImageIoError::Decode(format!("process: {e:?}")))?;
        processed_xyz8_to_decoded(&processed, label, q, Some(path.to_path_buf()))
    }
}

/// LibRaw linear **XYZ** (16-bit) → app working **linear sRGB**.
#[cfg(feature = "raw")]
fn processed_xyz16_to_decoded(
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
        let x = samples[base];
        let y = samples[base + 1];
        let z = if colors >= 3 {
            samples[base + 2]
        } else {
            samples[base + 1]
        };
        let [r, g, b] = color::libraw_xyz16_to_linear_srgb(x, y, z);
        rgba_f32.extend_from_slice(&[r, g, b, 1.0]);
    }

    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality,
        color: ColorMeta::from_libraw_linear_xyz(),
        // LibRaw dcraw_process already applies sizes.flip to the mem image.
        orientation: ImageOrientation::identity(),
        source_path,
    })
}

/// LibRaw linear **XYZ** (8-bit proxy) → app working **linear sRGB**.
#[cfg(feature = "raw")]
fn processed_xyz8_to_decoded(
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
        let x = samples[base];
        let y = samples[base + 1];
        let z = if colors >= 3 {
            samples[base + 2]
        } else {
            samples[base + 1]
        };
        let [r, g, b] = color::libraw_xyz8_to_linear_srgb(x, y, z);
        rgba_f32.extend_from_slice(&[r, g, b, 1.0]);
    }

    Ok(DecodedImage {
        width,
        height,
        rgba_f32,
        label,
        quality,
        color: ColorMeta::from_libraw_linear_xyz(),
        orientation: ImageOrientation::identity(),
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

/// Encode **linear sRGB working-space** RGBA f32 to display sRGB PNG/JPEG.
///
/// Assumes [`WorkingSpace::LinearSrgb`](crate::color::WorkingSpace::LinearSrgb)
/// (app policy after Phase 2 Option A).
pub fn save_srgb_image(
    path: &Path,
    width: u32,
    height: u32,
    linear_rgba: &[f32],
) -> Result<(), ImageIoError> {
    debug_assert!(
        // Documented contract; not checked per-pixel for speed
        true,
        "export expects linear sRGB working space"
    );

    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for chunk in linear_rgba.chunks_exact(4) {
        bytes.push(color::linear_srgb_to_u8(chunk[0]));
        bytes.push(color::linear_srgb_to_u8(chunk[1]));
        bytes.push(color::linear_srgb_to_u8(chunk[2]));
        bytes.push((chunk[3].clamp(0.0, 1.0) * 255.0).round() as u8);
    }

    let img = image::RgbaImage::from_raw(width, height, bytes)
        .ok_or_else(|| ImageIoError::Decode("failed to build export image buffer".into()))?;

    img.save(path)
        .map_err(|e| ImageIoError::Decode(e.to_string()))
}
