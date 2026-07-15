//! Non-destructive crop in normalized image UV space (0..1).
//! Source texture is never modified; crop is applied at present + export.

use serde::{Deserialize, Serialize};

/// Axis-aligned crop rectangle in **full-image** UV coordinates.
/// `(left, top)` is the min corner; `(right, bottom)` is exclusive max (0..1).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

impl CropRect {
    pub fn width(self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    pub fn height(self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }

    #[allow(dead_code)]
    pub fn aspect(self) -> f32 {
        let h = self.height();
        if h > 1e-6 {
            self.width() / h
        } else {
            1.0
        }
    }

    pub fn is_full_frame(self) -> bool {
        self.left <= 1e-5
            && self.top <= 1e-5
            && self.right >= 1.0 - 1e-5
            && self.bottom >= 1.0 - 1e-5
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Clamp to image and enforce minimum size (~2% of frame).
    pub fn sanitize(&mut self) {
        const MIN: f32 = 0.02;
        self.left = self.left.clamp(0.0, 1.0 - MIN);
        self.top = self.top.clamp(0.0, 1.0 - MIN);
        self.right = self.right.clamp(MIN, 1.0);
        self.bottom = self.bottom.clamp(MIN, 1.0);
        if self.right - self.left < MIN {
            self.right = (self.left + MIN).min(1.0);
            self.left = self.right - MIN;
        }
        if self.bottom - self.top < MIN {
            self.bottom = (self.top + MIN).min(1.0);
            self.top = self.bottom - MIN;
        }
    }

    /// Constrain to a target aspect ratio (width/height), keeping center fixed when possible.
    pub fn set_aspect_centered(&mut self, aspect: f32, image_aspect: f32) {
        // aspect here is crop width/height in **image UV space**.
        // UV x and y are not equal in world unless image is square — for photo crops
        // users think in pixel aspect: pixel_w/pixel_h = (uv_w * img_w) / (uv_h * img_h)
        // => uv_w/uv_h = aspect_pixels * (img_h/img_w) = aspect_pixels / image_aspect
        let _ = image_aspect;
        let target_uv_aspect = aspect; // we store pixel aspect separately via presets
        let cx = (self.left + self.right) * 0.5;
        let cy = (self.top + self.bottom) * 0.5;
        let mut w = self.width().max(0.02);
        let mut h = w / target_uv_aspect.max(1e-6);
        if h > 1.0 {
            h = 1.0;
            w = h * target_uv_aspect;
        }
        if w > 1.0 {
            w = 1.0;
            h = w / target_uv_aspect.max(1e-6);
        }
        self.left = (cx - w * 0.5).clamp(0.0, 1.0);
        self.right = (self.left + w).min(1.0);
        self.left = self.right - w;
        self.top = (cy - h * 0.5).clamp(0.0, 1.0);
        self.bottom = (self.top + h).min(1.0);
        self.top = self.bottom - h;
        self.sanitize();
    }

    /// Apply pixel-space aspect (w/h in pixels) given full image pixel aspect.
    pub fn set_pixel_aspect_centered(&mut self, pixel_aspect: f32, image_pixel_aspect: f32) {
        // uv_w / uv_h = pixel_aspect / image_pixel_aspect
        let uv_aspect = pixel_aspect / image_pixel_aspect.max(1e-6);
        self.set_aspect_centered(uv_aspect, image_pixel_aspect);
    }
}

/// Constrained crop aspect (Lightroom-style presets).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CropAspect {
    #[default]
    Free,
    Original,
    Square,
    /// 4:3
    R4x3,
    /// 3:2
    R3x2,
    /// 16:9
    R16x9,
}

impl CropAspect {
    pub fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Original => "Original",
            Self::Square => "1:1",
            Self::R4x3 => "4:3",
            Self::R3x2 => "3:2",
            Self::R16x9 => "16:9",
        }
    }

    /// Pixel aspect ratio width/height, or None if free/original handled by caller.
    pub fn pixel_ratio(self) -> Option<f32> {
        match self {
            Self::Free | Self::Original => None,
            Self::Square => Some(1.0),
            Self::R4x3 => Some(4.0 / 3.0),
            Self::R3x2 => Some(3.0 / 2.0),
            Self::R16x9 => Some(16.0 / 9.0),
        }
    }
}

/// Which part of the crop UI is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropHit {
    None,
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Full crop + rotate tool state (non-destructive), Lightroom-style.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CropState {
    pub rect: CropRect,
    pub aspect: CropAspect,
    /// Fine straighten angle in degrees (−45…45). Image rotates under the crop.
    pub angle_deg: f32,
    /// Quarter-turns counter-clockwise (0…3).
    pub orient_90: u32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// When true, canvas shows full image + overlay editor.
    #[serde(skip)]
    pub editing: bool,
}

impl Default for CropState {
    fn default() -> Self {
        Self {
            rect: CropRect::default(),
            aspect: CropAspect::Free,
            angle_deg: 0.0,
            orient_90: 0,
            flip_h: false,
            flip_v: false,
            editing: false,
        }
    }
}

impl CropState {
    pub fn reset(&mut self) {
        self.rect.reset();
        self.aspect = CropAspect::Free;
        self.angle_deg = 0.0;
        self.orient_90 = 0;
        self.flip_h = false;
        self.flip_v = false;
    }

    pub fn apply_aspect_preset(&mut self, image_pixel_aspect: f32) {
        match self.aspect {
            CropAspect::Free => {}
            CropAspect::Original => {
                self.rect.set_pixel_aspect_centered(image_pixel_aspect, image_pixel_aspect);
                // Original means full-frame usually; keep current center size max
                self.rect = CropRect::default();
            }
            other => {
                if let Some(r) = other.pixel_ratio() {
                    self.rect
                        .set_pixel_aspect_centered(r, image_pixel_aspect.max(1e-6));
                }
            }
        }
    }

    /// Rotate image 90° counter-clockwise (non-destructive).
    pub fn rotate_90_ccw(&mut self) {
        self.orient_90 = (self.orient_90 + 1) % 4;
        // Transform crop rect in UV to match (CCW 90 of content)
        // (u,v) -> (v, 1-u) for image CCW with y-down is wrong for crop corners...
        // Content CCW 90: top edge becomes left. Crop corners:
        // (L,T)->(T,1-L), (R,T)->(T,1-R), (L,B)->(B,1-L), (R,B)->(B,1-R)
        let l = self.rect.left;
        let t = self.rect.top;
        let r = self.rect.right;
        let b = self.rect.bottom;
        // After CCW image rotation, map old UV -> new UV is CW on the rect
        // new_u = old_v, new_v = 1-old_u  for CCW of image with y-down?
        // Image CCW 90: pixel (x,y) -> (y, W-1-x). UV (u,v)=(x/W,y/H) -> (v*H/W?, ...)
        // Keep it simple for UV square mapping (crop is in UV):
        // CCW: (u,v) -> (v, 1-u)
        let corners = [(l, t), (r, t), (l, b), (r, b)].map(|(u, v)| (v, 1.0 - u));
        let us: Vec<f32> = corners.iter().map(|c| c.0).collect();
        let vs: Vec<f32> = corners.iter().map(|c| c.1).collect();
        self.rect.left = us.iter().cloned().fold(1.0f32, f32::min);
        self.rect.right = us.iter().cloned().fold(0.0f32, f32::max);
        self.rect.top = vs.iter().cloned().fold(1.0f32, f32::min);
        self.rect.bottom = vs.iter().cloned().fold(0.0f32, f32::max);
        self.rect.sanitize();
    }

    /// Rotate image 90° clockwise.
    pub fn rotate_90_cw(&mut self) {
        // 3× CCW
        self.rotate_90_ccw();
        self.rotate_90_ccw();
        self.rotate_90_ccw();
    }

    pub fn toggle_flip_h(&mut self) {
        self.flip_h = !self.flip_h;
        let l = self.rect.left;
        let r = self.rect.right;
        self.rect.left = 1.0 - r;
        self.rect.right = 1.0 - l;
        self.rect.sanitize();
    }

    pub fn toggle_flip_v(&mut self) {
        self.flip_v = !self.flip_v;
        let t = self.rect.top;
        let b = self.rect.bottom;
        self.rect.top = 1.0 - b;
        self.rect.bottom = 1.0 - t;
        self.rect.sanitize();
    }

    /// Effective source aspect after 90° turns (W/H of how we display the photo).
    pub fn display_source_aspect(&self, full_aspect: f32) -> f32 {
        if self.orient_90 % 2 == 1 {
            1.0 / full_aspect.max(1e-6)
        } else {
            full_aspect
        }
    }
}

/// Map display-space UV (0..1, after crop) → source texture UV (inverse orient).
pub fn display_uv_to_source_uv(
    uv: (f32, f32),
    angle_deg: f32,
    orient_90: u32,
    flip_h: bool,
    flip_v: bool,
    source_aspect: f32,
) -> (f32, f32) {
    let mut u = uv.0;
    let mut v = uv.1;

    // Inverse fine rotation (display was rotated by +angle → sample with -angle)
    let rad = -angle_deg.to_radians();
    if rad.abs() > 1e-6 {
        let a = source_aspect.max(1e-6);
        let x = (u - 0.5) * a;
        let y = v - 0.5;
        let c = rad.cos();
        let s = rad.sin();
        let rx = x * c - y * s;
        let ry = x * s + y * c;
        u = rx / a + 0.5;
        v = ry + 0.5;
    }

    // Inverse orient_90 CCW on image = CW * orient_90 on UV
    // CW 90 (y-down): (u,v) -> (1-v, u)
    for _ in 0..(orient_90 % 4) {
        let nu = 1.0 - v;
        let nv = u;
        u = nu;
        v = nv;
    }

    if flip_h {
        u = 1.0 - u;
    }
    if flip_v {
        v = 1.0 - v;
    }

    (u, v)
}

/// Bilinear sample of linear RGBA image at UV (for export).
pub fn sample_rgba(rgba: &[f32], width: u32, height: u32, u: f32, v: f32) -> [f32; 4] {
    if u < 0.0 || u > 1.0 || v < 0.0 || v > 1.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let w = width as f32;
    let h = height as f32;
    let x = (u * (w - 1.0)).clamp(0.0, w - 1.001);
    let y = (v * (h - 1.0)).clamp(0.0, h - 1.001);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width as usize - 1);
    let y1 = (y0 + 1).min(height as usize - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let idx = |xx: usize, yy: usize| (yy * width as usize + xx) * 4;
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let v00 = rgba[idx(x0, y0) + c];
        let v10 = rgba[idx(x1, y0) + c];
        let v01 = rgba[idx(x0, y1) + c];
        let v11 = rgba[idx(x1, y1) + c];
        let v0 = v00 * (1.0 - fx) + v10 * fx;
        let v1 = v01 * (1.0 - fx) + v11 * fx;
        out[c] = v0 * (1.0 - fy) + v1 * fy;
    }
    out
}

/// Rasterize crop+rotate into a new buffer (export / bake). Output size = crop size in pixels.
pub fn render_crop_rotate(
    rgba: &[f32],
    width: u32,
    height: u32,
    crop: &CropState,
) -> (u32, u32, Vec<f32>) {
    let full_aspect = width as f32 / height.max(1) as f32;
    let src_aspect = full_aspect; // rotation uses original source aspect

    let cw = ((crop.rect.width() * width as f32).round() as u32).max(1);
    let ch = ((crop.rect.height() * height as f32).round() as u32).max(1);
    // After odd 90°, displayed aspect of full frame swaps — crop is in display UV of oriented image.
    // Our crop UV is stored in "display after orient" space if we transform crop on 90° clicks.
    // Fine angle applied in display space before inverse orient.

    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in 0..ch {
        for x in 0..cw {
            let u_disp = crop.rect.left + (x as f32 + 0.5) / cw as f32 * crop.rect.width();
            let v_disp = crop.rect.top + (y as f32 + 0.5) / ch as f32 * crop.rect.height();
            let (su, sv) = display_uv_to_source_uv(
                (u_disp, v_disp),
                crop.angle_deg,
                crop.orient_90,
                crop.flip_h,
                crop.flip_v,
                src_aspect,
            );
            let px = sample_rgba(rgba, width, height, su, sv);
            out.extend_from_slice(&px);
        }
    }
    let _ = full_aspect;
    (cw, ch, out)
}

/// Hit-test crop handles in **screen** space. `crop_screen` is the crop rectangle on screen.
pub fn hit_test(crop_screen: egui::Rect, pos: egui::Pos2, handle_px: f32) -> CropHit {
    let r = crop_screen;
    let h = handle_px;

    let near_l = (pos.x - r.left()).abs() <= h;
    let near_r = (pos.x - r.right()).abs() <= h;
    let near_t = (pos.y - r.top()).abs() <= h;
    let near_b = (pos.y - r.bottom()).abs() <= h;
    let in_x = pos.x >= r.left() - h && pos.x <= r.right() + h;
    let in_y = pos.y >= r.top() - h && pos.y <= r.bottom() + h;

    if near_t && near_l {
        return CropHit::TopLeft;
    }
    if near_t && near_r {
        return CropHit::TopRight;
    }
    if near_b && near_l {
        return CropHit::BottomLeft;
    }
    if near_b && near_r {
        return CropHit::BottomRight;
    }
    if near_l && in_y {
        return CropHit::Left;
    }
    if near_r && in_y {
        return CropHit::Right;
    }
    if near_t && in_x {
        return CropHit::Top;
    }
    if near_b && in_x {
        return CropHit::Bottom;
    }
    if r.contains(pos) {
        return CropHit::Move;
    }
    CropHit::None
}

/// Apply a drag in **image UV** delta (dx, dy).
pub fn apply_drag(
    rect: &mut CropRect,
    hit: CropHit,
    d_uv: egui::Vec2,
    aspect: CropAspect,
    image_pixel_aspect: f32,
) {
    match hit {
        CropHit::None => {}
        CropHit::Move => {
            let w = rect.width();
            let h = rect.height();
            rect.left = (rect.left + d_uv.x).clamp(0.0, 1.0 - w);
            rect.top = (rect.top + d_uv.y).clamp(0.0, 1.0 - h);
            rect.right = rect.left + w;
            rect.bottom = rect.top + h;
        }
        CropHit::Left => {
            rect.left = (rect.left + d_uv.x).clamp(0.0, rect.right - 0.02);
            constrain_edge(rect, aspect, image_pixel_aspect, true, false);
        }
        CropHit::Right => {
            rect.right = (rect.right + d_uv.x).clamp(rect.left + 0.02, 1.0);
            constrain_edge(rect, aspect, image_pixel_aspect, true, false);
        }
        CropHit::Top => {
            rect.top = (rect.top + d_uv.y).clamp(0.0, rect.bottom - 0.02);
            constrain_edge(rect, aspect, image_pixel_aspect, false, true);
        }
        CropHit::Bottom => {
            rect.bottom = (rect.bottom + d_uv.y).clamp(rect.top + 0.02, 1.0);
            constrain_edge(rect, aspect, image_pixel_aspect, false, true);
        }
        CropHit::TopLeft => {
            rect.left = (rect.left + d_uv.x).clamp(0.0, rect.right - 0.02);
            rect.top = (rect.top + d_uv.y).clamp(0.0, rect.bottom - 0.02);
            constrain_corner(rect, aspect, image_pixel_aspect);
        }
        CropHit::TopRight => {
            rect.right = (rect.right + d_uv.x).clamp(rect.left + 0.02, 1.0);
            rect.top = (rect.top + d_uv.y).clamp(0.0, rect.bottom - 0.02);
            constrain_corner(rect, aspect, image_pixel_aspect);
        }
        CropHit::BottomLeft => {
            rect.left = (rect.left + d_uv.x).clamp(0.0, rect.right - 0.02);
            rect.bottom = (rect.bottom + d_uv.y).clamp(rect.top + 0.02, 1.0);
            constrain_corner(rect, aspect, image_pixel_aspect);
        }
        CropHit::BottomRight => {
            rect.right = (rect.right + d_uv.x).clamp(rect.left + 0.02, 1.0);
            rect.bottom = (rect.bottom + d_uv.y).clamp(rect.top + 0.02, 1.0);
            constrain_corner(rect, aspect, image_pixel_aspect);
        }
    }
    rect.sanitize();
}

/// UV width/height ratio for a locked pixel aspect.
fn target_uv_aspect(aspect: CropAspect, image_pixel_aspect: f32) -> Option<f32> {
    match aspect {
        CropAspect::Free => None,
        // Same shape as full image ⇒ UV aspect = 1
        CropAspect::Original => Some(1.0),
        other => other
            .pixel_ratio()
            .map(|pr| pr / image_pixel_aspect.max(1e-6)),
    }
}

fn constrain_edge(
    rect: &mut CropRect,
    aspect: CropAspect,
    image_pixel_aspect: f32,
    horizontal_edge: bool,
    _vertical_edge: bool,
) {
    let Some(uv_a) = target_uv_aspect(aspect, image_pixel_aspect) else {
        return;
    };

    if horizontal_edge {
        // width changed → adjust height around center
        let w = rect.width();
        let h = w / uv_a.max(1e-6);
        let cy = (rect.top + rect.bottom) * 0.5;
        rect.top = (cy - h * 0.5).clamp(0.0, 1.0);
        rect.bottom = (rect.top + h).min(1.0);
        if rect.bottom - rect.top < h - 1e-5 {
            rect.top = (rect.bottom - h).max(0.0);
        }
    } else {
        let h = rect.height();
        let w = h * uv_a;
        let cx = (rect.left + rect.right) * 0.5;
        rect.left = (cx - w * 0.5).clamp(0.0, 1.0);
        rect.right = (rect.left + w).min(1.0);
        if rect.right - rect.left < w - 1e-5 {
            rect.left = (rect.right - w).max(0.0);
        }
    }
}

fn constrain_corner(rect: &mut CropRect, aspect: CropAspect, image_pixel_aspect: f32) {
    let Some(uv_a) = target_uv_aspect(aspect, image_pixel_aspect) else {
        return;
    };
    // Keep width, recompute height from left/top anchor
    let w = rect.width();
    let h = w / uv_a.max(1e-6);
    rect.bottom = (rect.top + h).min(1.0);
    if rect.bottom - rect.top < h - 1e-5 {
        rect.top = (rect.bottom - h).max(0.0);
    }
}

/// Map full-image UV → screen position given fitted image rect on screen (no pan/zoom).
pub fn image_uv_to_screen(uv: egui::Pos2, image_rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        image_rect.left() + uv.x * image_rect.width(),
        image_rect.top() + uv.y * image_rect.height(),
    )
}

#[allow(dead_code)]
pub fn screen_to_image_uv(pos: egui::Pos2, image_rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        ((pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0),
        ((pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0),
    )
}

/// Letterboxed/pillarboxed rectangle where the full image is drawn (fit mode).
pub fn fitted_image_rect(viewport: egui::Rect, image_aspect: f32) -> egui::Rect {
    let view_a = viewport.width() / viewport.height().max(1.0);
    let img_a = image_aspect.max(1e-6);
    if view_a > img_a {
        // pillarbox
        let h = viewport.height();
        let w = h * img_a;
        let x = viewport.center().x - w * 0.5;
        egui::Rect::from_min_size(egui::pos2(x, viewport.top()), egui::vec2(w, h))
    } else {
        // letterbox
        let w = viewport.width();
        let h = w / img_a;
        let y = viewport.center().y - h * 0.5;
        egui::Rect::from_min_size(egui::pos2(viewport.left(), y), egui::vec2(w, h))
    }
}

/// Crop a linear RGBA buffer (full image) to `rect` (non-destructive source kept separately).
#[allow(dead_code)]
pub fn crop_pixels(
    rgba: &[f32],
    width: u32,
    height: u32,
    rect: CropRect,
) -> (u32, u32, Vec<f32>) {
    let w = width as usize;
    let h = height as usize;
    let x0 = ((rect.left * width as f32).floor() as usize).min(w.saturating_sub(1));
    let y0 = ((rect.top * height as f32).floor() as usize).min(h.saturating_sub(1));
    let x1 = ((rect.right * width as f32).ceil() as usize).clamp(x0 + 1, w);
    let y1 = ((rect.bottom * height as f32).ceil() as usize).clamp(y0 + 1, h);
    let cw = x1 - x0;
    let ch = y1 - y0;
    let mut out = Vec::with_capacity(cw * ch * 4);
    for y in y0..y1 {
        let row = y * w * 4;
        for x in x0..x1 {
            let i = row + x * 4;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    (cw as u32, ch as u32, out)
}
