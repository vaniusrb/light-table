//! Develop parameters shared between CPU (egui) and GPU (uniform buffer).
//!
//! Layout of [`DevelopGpuParams`] must match `shader/src/lib.rs` exactly.

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// GPU uniform buffer for develop + view. `#[repr(C)]` + Pod for bytemuck upload.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct DevelopGpuParams {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub temperature: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub image_aspect: f32,
    pub view_aspect: f32,
    pub has_image: u32,
    pub denoise_luma: f32,
    pub denoise_chroma: f32,
    pub texel_w: f32,
    pub texel_h: f32,
    pub sharpen_amount: f32,
    pub sharpen_radius: f32,
    pub sharpen_detail: f32,
    pub _pad_sharp: f32,
    /// Crop rect in full-image UV (non-destructive).
    pub crop_left: f32,
    pub crop_top: f32,
    pub crop_right: f32,
    pub crop_bottom: f32,
    /// 1 = crop edit mode (show full image, dim outside crop).
    pub crop_edit: u32,
    /// UI content area in full-window screen UV (0..1). Image is letterboxed inside this.
    /// Keeps the photo (and crop grid) out from under the side panel / toolbar.
    pub content_left: f32,
    pub content_top: f32,
    pub content_right: f32,
    pub content_bottom: f32,
    /// Fine straighten angle in **radians** (image rotates under crop).
    pub rotate_angle: f32,
    /// Quarter-turns CCW (0..3).
    pub orient_90: u32,
    pub flip_h: u32,
    pub flip_v: u32,
    /// Full source texture aspect W/H (for aspect-correct rotation).
    pub source_aspect: f32,
    pub _pad_rot0: f32,
    pub _pad_rot1: f32,
    pub _pad_rot2: f32,
}

/// User-editable develop settings (egui + optional JSON sidecar).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevelopParams {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub temperature: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    /// Luminance noise reduction 0..1 (edge-aware).
    pub denoise_luma: f32,
    /// Color noise reduction 0..1.
    pub denoise_chroma: f32,
    /// Unsharp mask amount 0..2.
    pub sharpen_amount: f32,
    /// Unsharp blur radius in pixels (~0.5..3).
    pub sharpen_radius: f32,
    /// Mask flat areas 0..1 (higher = less noise amplification).
    pub sharpen_detail: f32,
}

impl Default for DevelopParams {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temperature: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            denoise_luma: 0.0,
            denoise_chroma: 0.0,
            sharpen_amount: 0.0,
            sharpen_radius: 1.0,
            sharpen_detail: 0.25,
        }
    }
}

impl DevelopParams {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Canvas pan/zoom state (CPU only; packed into GPU params each frame).
#[derive(Clone, Debug)]
pub struct ViewState {
    pub pan_x: f32,
    pub pan_y: f32,
    /// 1.0 = fit image in view.
    pub zoom: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
        }
    }
}

impl ViewState {
    pub fn fit(&mut self) {
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.zoom = 1.0;
    }
}

/// UI content rectangle in full-window **normalized** coordinates (0..1),
/// plus the content area's true **pixel** aspect (width/height in points).
///
/// Note: `(right-left)/(bottom-top)` in UV space is **not** the pixel aspect
/// unless the window is square — always use [`ContentViewport::pixel_aspect`].
#[derive(Clone, Copy, Debug)]
pub struct ContentViewport {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// content_rect.width() / content_rect.height() in screen points/pixels
    pub pixel_aspect: f32,
}

impl Default for ContentViewport {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
            pixel_aspect: 16.0 / 9.0,
        }
    }
}

impl ContentViewport {
    pub fn from_rects(screen: egui::Rect, content: egui::Rect) -> Self {
        let sw = screen.width().max(1.0);
        let sh = screen.height().max(1.0);
        let cw = content.width().max(1.0);
        let ch = content.height().max(1.0);
        Self {
            left: ((content.left() - screen.left()) / sw).clamp(0.0, 1.0),
            top: ((content.top() - screen.top()) / sh).clamp(0.0, 1.0),
            right: ((content.right() - screen.left()) / sw).clamp(0.0, 1.0),
            bottom: ((content.bottom() - screen.top()) / sh).clamp(0.0, 1.0),
            // True geometric aspect of the content area (fixes horizontal stretch)
            pixel_aspect: cw / ch,
        }
    }

    pub fn aspect(self) -> f32 {
        if self.pixel_aspect > 1e-6 {
            self.pixel_aspect
        } else {
            1.0
        }
    }
}

/// Build the GPU uniform from develop + view + crop + image metadata.
pub fn build_gpu_params(
    develop: &DevelopParams,
    view: &ViewState,
    crop: &crate::crop::CropState,
    content: ContentViewport,
    image_width: u32,
    image_height: u32,
    has_image: bool,
) -> DevelopGpuParams {
    // In normal view, letterbox to the **cropped** aspect so the frame fills the crop.
    // In crop-edit mode, letterbox to the full image so the user sees the whole photo.
    let full_aspect = if image_height > 0 {
        image_width as f32 / image_height as f32
    } else {
        1.0
    };
    // After 90° turns, the *displayed* full-frame aspect swaps
    let display_full_aspect = crop.display_source_aspect(full_aspect);
    let crop_aspect = {
        let cw = crop.rect.width().max(1e-6);
        let ch = crop.rect.height().max(1e-6);
        // crop is stored in display/oriented UV space
        (cw / ch) * display_full_aspect
    };
    let image_aspect = if crop.editing {
        display_full_aspect
    } else {
        crop_aspect
    };
    // Letterbox inside the UI content area (not the full window under the side panel)
    let view_aspect = content.aspect();

    let texel_w = if image_width > 0 {
        1.0 / image_width as f32
    } else {
        0.0
    };
    let texel_h = if image_height > 0 {
        1.0 / image_height as f32
    } else {
        0.0
    };

    DevelopGpuParams {
        exposure: develop.exposure,
        contrast: develop.contrast,
        highlights: develop.highlights,
        shadows: develop.shadows,
        whites: develop.whites,
        blacks: develop.blacks,
        temperature: develop.temperature,
        tint: develop.tint,
        vibrance: develop.vibrance,
        saturation: develop.saturation,
        pan_x: view.pan_x,
        pan_y: view.pan_y,
        zoom: view.zoom,
        image_aspect,
        view_aspect,
        has_image: if has_image { 1 } else { 0 },
        denoise_luma: develop.denoise_luma,
        denoise_chroma: develop.denoise_chroma,
        texel_w,
        texel_h,
        sharpen_amount: develop.sharpen_amount,
        sharpen_radius: develop.sharpen_radius,
        sharpen_detail: develop.sharpen_detail,
        _pad_sharp: 0.0,
        crop_left: crop.rect.left,
        crop_top: crop.rect.top,
        crop_right: crop.rect.right,
        crop_bottom: crop.rect.bottom,
        crop_edit: if crop.editing { 1 } else { 0 },
        content_left: content.left,
        content_top: content.top,
        content_right: content.right,
        content_bottom: content.bottom,
        rotate_angle: crop.angle_deg.to_radians(),
        orient_90: crop.orient_90 % 4,
        flip_h: if crop.flip_h { 1 } else { 0 },
        flip_v: if crop.flip_v { 1 } else { 0 },
        // Always original texture W/H for sampling rotation math
        source_aspect: full_aspect,
        _pad_rot0: 0.0,
        _pad_rot1: 0.0,
        _pad_rot2: 0.0,
    }
}
