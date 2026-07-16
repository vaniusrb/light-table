//! GPU shaders for light-table — pure Rust via rust-gpu → SPIR-V.
//!
//! Entry points:
//!   - `vs_present` / `fs_present` — pan/zoom canvas with parametric develop
//!   - `hist_cs` — RGB/luma histogram bins (atomics)

#![no_std]

use spirv_std::glam::{vec2, vec3, vec4, Vec2, Vec3, Vec4};
use spirv_std::image::Image2d;
use spirv_std::{spirv, Sampler};

// ---------------------------------------------------------------------------
// Shared uniforms — must match CPU `DevelopGpuParams` layout exactly.
// ---------------------------------------------------------------------------

/// Parametric develop + view transform (uploaded every frame when dirty).
#[repr(C)]
#[derive(Copy, Clone)]
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
    /// Pan offset in image UV space (0 = centered).
    pub pan_x: f32,
    pub pan_y: f32,
    /// Zoom scale (1.0 = fit).
    pub zoom: f32,
    pub image_aspect: f32,
    pub view_aspect: f32,
    pub has_image: u32,
    /// Luminance denoise strength 0..1 (edge-aware bilateral).
    pub denoise_luma: f32,
    /// Color denoise strength 0..1 (wider chroma blur).
    pub denoise_chroma: f32,
    /// 1 / source texture width (UV step for 1 pixel).
    pub texel_w: f32,
    /// 1 / source texture height.
    pub texel_h: f32,
    /// Unsharp mask amount 0..2 (0 = off).
    pub sharpen_amount: f32,
    /// Blur radius in pixels for unsharp mask (~0.5..3).
    pub sharpen_radius: f32,
    /// Detail threshold 0..1 — higher ignores flat/noisy areas.
    pub sharpen_detail: f32,
    pub _pad_sharp: f32,
    /// Non-destructive crop in full-image UV.
    pub crop_left: f32,
    pub crop_top: f32,
    pub crop_right: f32,
    pub crop_bottom: f32,
    /// 1 = crop edit (show full image; dim outside crop).
    pub crop_edit: u32,
    /// UI content area in full-window screen UV (image is drawn only here).
    pub content_left: f32,
    pub content_top: f32,
    pub content_right: f32,
    pub content_bottom: f32,
    /// Fine straighten angle in radians (display rotation; sampling uses inverse).
    pub rotate_angle: f32,
    /// Quarter-turns CCW applied to the image (0..3).
    pub orient_90: u32,
    pub flip_h: u32,
    pub flip_v: u32,
    /// Source texture aspect W/H.
    pub source_aspect: f32,
    pub _pad_rot0: f32,
    pub _pad_rot1: f32,
    pub _pad_rot2: f32,
}

// ---------------------------------------------------------------------------
// Color helpers (linear light)
// ---------------------------------------------------------------------------

#[inline]
fn f32_powf(x: f32, e: f32) -> f32 {
    use spirv_std::num_traits::Float;
    x.powf(e)
}

/// IEC 61966-2-1 sRGB transfer (linear → non-linear). Continuous at the break.
/// The old sqrt-blend approx jumped at 0.0031308 (≈0.040 vs ≈0.067), which
/// starved histogram bins around ~8–17 and looked like a “dip” near index 8.
#[inline]
fn linear_to_srgb_channel(c: f32) -> f32 {
    let c = f32_clamp(c, 0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * f32_powf(c, 1.0 / 2.4) - 0.055
    }
}

/// Map display-referred [0,1] → histogram bin 0..255 without truncating mid-bin mass.
#[inline]
fn display_to_bin(c: f32) -> u32 {
    let c = f32_clamp(c, 0.0, 1.0);
    // floor(c * 255.999) keeps c=1.0 in bin 255 and avoids trunc bias toward lower bins
    let b = c * 255.999;
    f32_min(b, 255.0) as u32
}

#[inline]
fn f32_abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

#[inline]
fn f32_max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

#[inline]
fn f32_min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn f32_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    f32_min(f32_max(x, lo), hi)
}

#[inline]
fn exp2_approx(x: f32) -> f32 {
    // 2^x for exposure; x typically in [-5, 5]
    use spirv_std::num_traits::Float;
    x.exp2()
}

/// Apply white-balance style RGB gains from temperature/tint (-1..1).
#[inline]
fn apply_wb(rgb: Vec3, temperature: f32, tint: f32) -> Vec3 {
    // Warm: boost R, reduce B; cool: opposite. Tint: green ↔ magenta.
    let r_gain = 1.0 + temperature * 0.35 - tint * 0.08;
    let g_gain = 1.0 + tint * 0.20;
    let b_gain = 1.0 - temperature * 0.35 - tint * 0.08;
    vec3(rgb.x * r_gain, rgb.y * g_gain, rgb.z * b_gain)
}

#[inline]
fn apply_exposure(rgb: Vec3, exposure: f32) -> Vec3 {
    let m = exp2_approx(exposure);
    rgb * m
}

#[inline]
fn apply_contrast(rgb: Vec3, contrast: f32) -> Vec3 {
    // contrast: -1..1, pivot mid-grey 0.18 in linear (approx)
    let pivot = 0.18;
    let c = 1.0 + contrast;
    (rgb - vec3(pivot, pivot, pivot)) * c + vec3(pivot, pivot, pivot)
}

#[inline]
fn luminance(rgb: Vec3) -> f32 {
    rgb.x * 0.2126 + rgb.y * 0.7152 + rgb.z * 0.0722
}

/// Highlights / shadows lift around midtones (simple parametric).
#[inline]
fn apply_tone_region(rgb: Vec3, highlights: f32, shadows: f32, whites: f32, blacks: f32) -> Vec3 {
    let luma = luminance(rgb);
    // Shadows: affect dark regions more
    let shadow_w = f32_clamp(1.0 - luma * 2.0, 0.0, 1.0);
    let highlight_w = f32_clamp((luma - 0.5) * 2.0, 0.0, 1.0);
    let mut out = rgb;
    out += vec3(shadows, shadows, shadows) * shadow_w * 0.25;
    out += vec3(highlights, highlights, highlights) * highlight_w * 0.25;
    // Whites/blacks endpoints
    out = out * (1.0 + whites * 0.15) + vec3(blacks, blacks, blacks) * 0.1;
    out
}

#[inline]
fn apply_vibrance_sat(rgb: Vec3, vibrance: f32, saturation: f32) -> Vec3 {
    let luma = luminance(rgb);
    let grey = vec3(luma, luma, luma);
    // Vibrance: more effect on less-saturated pixels
    let max_c = f32_max(rgb.x, f32_max(rgb.y, rgb.z));
    let min_c = f32_min(rgb.x, f32_min(rgb.y, rgb.z));
    let sat = if max_c > 1e-5 {
        (max_c - min_c) / max_c
    } else {
        0.0
    };
    let vib_amount = vibrance * (1.0 - sat);
    let mut out = grey + (rgb - grey) * (1.0 + vib_amount);
    out = grey + (out - grey) * (1.0 + saturation);
    out
}

/// Full develop chain: linear RGB in → display-referred linear out (before sRGB encode).
#[inline]
fn develop_pixel(rgb: Vec3, p: &DevelopGpuParams) -> Vec3 {
    let mut c = apply_wb(rgb, p.temperature, p.tint);
    c = apply_exposure(c, p.exposure);
    c = apply_contrast(c, p.contrast);
    c = apply_tone_region(c, p.highlights, p.shadows, p.whites, p.blacks);
    c = apply_vibrance_sat(c, p.vibrance, p.saturation);
    // Soft clamp for preview
    vec3(
        f32_max(c.x, 0.0),
        f32_max(c.y, 0.0),
        f32_max(c.z, 0.0),
    )
}

#[inline]
fn f32_exp(x: f32) -> f32 {
    use spirv_std::num_traits::Float;
    x.exp()
}

/// Edge-aware denoise in linear light (before tone).
/// - Luma: 5×5 bilateral (preserves edges)
/// - Chroma: wider spatial blend toward neighborhood average
#[inline]
fn denoise_sample(
    source: &Image2d,
    sampler: &Sampler,
    uv: Vec2,
    p: &DevelopGpuParams,
) -> Vec3 {
    let center_s: Vec4 = source.sample(*sampler, uv);
    let center = vec3(center_s.x, center_s.y, center_s.z);

    let luma_str = f32_clamp(p.denoise_luma, 0.0, 1.0);
    let chroma_str = f32_clamp(p.denoise_chroma, 0.0, 1.0);
    if luma_str < 0.001 && chroma_str < 0.001 {
        return center;
    }

    let tw = if p.texel_w > 0.0 { p.texel_w } else { 1.0 / 1024.0 };
    let th = if p.texel_h > 0.0 { p.texel_h } else { 1.0 / 1024.0 };

    // Strength → filter sigmas (in pixel / linear-luma units)
    let sigma_s = 0.6 + luma_str * 2.2; // spatial
    let sigma_r = 0.02 + luma_str * 0.12; // range (linear luma)
    let inv_2s2 = 1.0 / (2.0 * sigma_s * sigma_s);
    let inv_2r2 = 1.0 / (2.0 * sigma_r * sigma_r);

    let center_l = luminance(center);
    let mut sum_l = 0.0_f32;
    let mut w_l = 0.0_f32;
    let mut sum_rgb = vec3(0.0, 0.0, 0.0);
    let mut w_c = 0.0_f32;

    // Radius 2 → 5×5 (fixed bounds for rust-gpu)
    let mut dy: i32 = -2;
    while dy <= 2 {
        let mut dx: i32 = -2;
        while dx <= 2 {
            let nuv = vec2(uv.x + dx as f32 * tw, uv.y + dy as f32 * th);
            // Clamp to image (avoid bleeding from letterbox via clamp sampler)
            let nuv = vec2(f32_clamp(nuv.x, 0.0, 1.0), f32_clamp(nuv.y, 0.0, 1.0));
            let ns: Vec4 = source.sample(*sampler, nuv);
            let n = vec3(ns.x, ns.y, ns.z);
            let nl = luminance(n);

            let d2 = (dx * dx + dy * dy) as f32;
            let ws = f32_exp(-d2 * inv_2s2);
            let dl = nl - center_l;
            let wr = f32_exp(-(dl * dl) * inv_2r2);
            let w = ws * wr;

            sum_l += nl * w;
            w_l += w;

            // Chroma blur: spatial only, slightly wider weight
            let wc = f32_exp(-d2 * inv_2s2 * 0.45);
            sum_rgb += n * wc;
            w_c += wc;

            dx += 1;
        }
        dy += 1;
    }

    let filt_l = if w_l > 1e-6 { sum_l / w_l } else { center_l };
    let filt_rgb = if w_c > 1e-6 {
        sum_rgb * (1.0 / w_c)
    } else {
        center
    };

    // Recombine: mix luma toward bilateral; mix chroma toward blurred RGB's chroma
    let center_chroma = center - vec3(center_l, center_l, center_l);
    let filt_chroma_l = luminance(filt_rgb);
    let filt_chroma = filt_rgb - vec3(filt_chroma_l, filt_chroma_l, filt_chroma_l);

    let out_l = center_l * (1.0 - luma_str) + filt_l * luma_str;
    let out_chroma = center_chroma * (1.0 - chroma_str) + filt_chroma * chroma_str;

    vec3(out_l, out_l, out_l) + out_chroma
}

/// 5×5 gaussian blur of source (weights use continuous radius).
#[inline]
fn gaussian_blur_sample(
    source: &Image2d,
    sampler: &Sampler,
    uv: Vec2,
    tw: f32,
    th: f32,
    radius: f32,
) -> Vec3 {
    let sigma = f32_max(radius * 0.5, 0.35);
    let inv_2s2 = 1.0 / (2.0 * sigma * sigma);
    let mut sum = vec3(0.0, 0.0, 0.0);
    let mut wsum = 0.0_f32;

    let mut dy: i32 = -2;
    while dy <= 2 {
        let mut dx: i32 = -2;
        while dx <= 2 {
            // Scale offsets by radius (radius 1 ≈ 1px steps)
            let ox = dx as f32 * radius * 0.5;
            let oy = dy as f32 * radius * 0.5;
            let nuv = vec2(
                f32_clamp(uv.x + ox * tw, 0.0, 1.0),
                f32_clamp(uv.y + oy * th, 0.0, 1.0),
            );
            let s: Vec4 = source.sample(*sampler, nuv);
            let d2 = ox * ox + oy * oy;
            let w = f32_exp(-d2 * inv_2s2);
            sum += vec3(s.x, s.y, s.z) * w;
            wsum += w;
            dx += 1;
        }
        dy += 1;
    }

    if wsum > 1e-6 {
        sum * (1.0 / wsum)
    } else {
        let s: Vec4 = source.sample(*sampler, uv);
        vec3(s.x, s.y, s.z)
    }
}

/// Unsharp mask on luma (after denoise). Amount 0 = passthrough.
#[inline]
fn sharpen_sample(center: Vec3, blur: Vec3, p: &DevelopGpuParams) -> Vec3 {
    let amount = f32_clamp(p.sharpen_amount, 0.0, 2.0);
    if amount < 0.001 {
        return center;
    }

    let cl = luminance(center);
    let bl = luminance(blur);
    let mut detail = cl - bl;

    // Detail / masking: suppress micro-contrast below threshold (reduces noise boost)
    let thr = f32_clamp(p.sharpen_detail, 0.0, 1.0) * 0.04;
    let ad = f32_abs(detail);
    if ad < thr {
        // Soft knee toward zero
        detail *= ad / f32_max(thr, 1e-6);
    }

    let out_l = cl + detail * amount;
    let chroma = center - vec3(cl, cl, cl);
    vec3(out_l, out_l, out_l) + chroma
}

/// Map screen UV → full-image UV.
///
/// Second return: `true` if this pixel is **outside the active viewport frame**
/// (letterbox / zoomed-out area). In normal mode the crop *is* the viewport, so
/// zooming out must not reveal pixels outside the crop.
///
/// - `crop_edit == 0`: local 0..1 maps into the crop rect only.
/// - `crop_edit == 1`: local 0..1 maps across the full image (crop tool).
#[inline]
fn screen_to_image_uv(screen_uv: Vec2, p: &DevelopGpuParams) -> (Vec2, bool) {
    // Restrict drawing to the UI content area (excludes toolbar + side panel)
    let c_l = p.content_left;
    let c_t = p.content_top;
    let c_r = if p.content_right > p.content_left {
        p.content_right
    } else {
        1.0
    };
    let c_b = if p.content_bottom > p.content_top {
        p.content_bottom
    } else {
        1.0
    };
    let c_w = f32_max(c_r - c_l, 1e-6);
    let c_h = f32_max(c_b - c_t, 1e-6);

    if screen_uv.x < c_l
        || screen_uv.x > c_r
        || screen_uv.y < c_t
        || screen_uv.y > c_b
    {
        return (vec2(0.0, 0.0), true);
    }

    // Screen UV → content-local 0..1
    let content_uv = vec2((screen_uv.x - c_l) / c_w, (screen_uv.y - c_t) / c_h);

    // Centered coords (-0.5..0.5) inside content area
    let mut c = content_uv - vec2(0.5, 0.5);

    // Letterbox/pillarbox to preserve image aspect inside content
    let img_a = if p.image_aspect > 0.0 {
        p.image_aspect
    } else {
        1.0
    };
    let view_a = if p.view_aspect > 0.0 {
        p.view_aspect
    } else {
        1.0
    };

    if view_a > img_a {
        c.x *= view_a / img_a;
    } else {
        c.y *= img_a / view_a;
    }

    let z = if p.zoom > 0.001 { p.zoom } else { 1.0 };
    c = c / z;
    c.x += p.pan_x;
    c.y += p.pan_y;

    let local = c + vec2(0.5, 0.5); // 0..1 over fitted image frame
    let outside_frame =
        local.x < 0.0 || local.x > 1.0 || local.y < 0.0 || local.y > 1.0;

    if p.crop_edit != 0 {
        return (local, outside_frame);
    }

    // Normal view: crop is the final viewport
    let cl = p.crop_left;
    let ct = p.crop_top;
    let cw = f32_max(p.crop_right - p.crop_left, 1e-6);
    let ch = f32_max(p.crop_bottom - p.crop_top, 1e-6);
    let lx = f32_clamp(local.x, 0.0, 1.0);
    let ly = f32_clamp(local.y, 0.0, 1.0);
    let uv = vec2(cl + lx * cw, ct + ly * ch);
    (uv, outside_frame)
}

/// Map display UV (crop/oriented space) → source texture UV.
/// Inverse of: flip → rot90 CCW × n → fine rotate.
#[inline]
fn display_to_source_uv(uv: Vec2, p: &DevelopGpuParams) -> Vec2 {
    let mut u = uv.x;
    let mut v = uv.y;

    // Inverse fine rotation around center (aspect-correct in pixel space)
    let ang = -p.rotate_angle;
    if f32_abs(ang) > 1e-6 {
        use spirv_std::num_traits::Float;
        let a = if p.source_aspect > 1e-6 {
            p.source_aspect
        } else {
            1.0
        };
        let x = (u - 0.5) * a;
        let y = v - 0.5;
        let c = ang.cos();
        let s = ang.sin();
        let rx = x * c - y * s;
        let ry = x * s + y * c;
        u = rx / a + 0.5;
        v = ry + 0.5;
    }

    // Inverse of n× CCW image rotation = n× CW on UV: (u,v) -> (1-v, u)
    let n = p.orient_90;
    let mut i = 0u32;
    while i < n {
        let nu = 1.0 - v;
        let nv = u;
        u = nu;
        v = nv;
        i += 1;
    }

    if p.flip_h != 0 {
        u = 1.0 - u;
    }
    if p.flip_v != 0 {
        v = 1.0 - v;
    }

    vec2(u, v)
}

// ---------------------------------------------------------------------------
// Present: fullscreen triangle + develop
// ---------------------------------------------------------------------------

#[spirv(vertex)]
pub fn vs_present(
    #[spirv(vertex_index)] vertex_id: i32,
    #[spirv(position)] out_pos: &mut Vec4,
    out_uv: &mut Vec2,
) {
    // Fullscreen triangle covering NDC [-1,1]
    let x = ((vertex_id & 1) * 4 - 1) as f32;
    let y = ((vertex_id & 2) * 2 - 1) as f32;
    *out_pos = vec4(x, y, 0.0, 1.0);
    *out_uv = vec2(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
}

#[spirv(fragment)]
pub fn fs_present(
    in_uv: Vec2,
    #[spirv(descriptor_set = 0, binding = 0)] source: &Image2d,
    #[spirv(descriptor_set = 0, binding = 1)] sampler: &Sampler,
    #[spirv(uniform, descriptor_set = 0, binding = 2)] params: &DevelopGpuParams,
    output: &mut Vec4,
) {
    if params.has_image == 0 {
        *output = vec4(0.08, 0.08, 0.10, 1.0);
        return;
    }

    // display_uv: oriented/crop space (axis-aligned crop + straighten view)
    let (display_uv, outside_frame) = screen_to_image_uv(in_uv, params);

    if outside_frame
        || display_uv.x < 0.0
        || display_uv.x > 1.0
        || display_uv.y < 0.0
        || display_uv.y > 1.0
    {
        *output = vec4(0.06, 0.06, 0.07, 1.0);
        return;
    }

    // Hard clip to crop rect in normal mode
    if params.crop_edit == 0
        && (display_uv.x < params.crop_left
            || display_uv.x > params.crop_right
            || display_uv.y < params.crop_top
            || display_uv.y > params.crop_bottom)
    {
        *output = vec4(0.06, 0.06, 0.07, 1.0);
        return;
    }

    // Inverse rotate / orient / flip → source texture UV
    let uv = display_to_source_uv(display_uv, params);

    // Outside source after rotation (straighten reveals empty corners)
    let outside_src = uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0;
    if outside_src {
        *output = vec4(0.06, 0.06, 0.07, 1.0);
        return;
    }

    // Denoise → sharpen in linear light, then parametric develop
    let denoised = denoise_sample(source, sampler, uv, params);
    let tw = if params.texel_w > 0.0 {
        params.texel_w
    } else {
        1.0 / 1024.0
    };
    let th = if params.texel_h > 0.0 {
        params.texel_h
    } else {
        1.0 / 1024.0
    };
    let radius = f32_clamp(params.sharpen_radius, 0.3, 3.0);
    let blur = if params.sharpen_amount > 0.001 {
        gaussian_blur_sample(source, sampler, uv, tw, th, radius)
    } else {
        denoised
    };
    let linear = sharpen_sample(denoised, blur, params);
    let developed = develop_pixel(linear, params);

    let r = linear_to_srgb_channel(developed.x);
    let g = linear_to_srgb_channel(developed.y);
    let b = linear_to_srgb_channel(developed.z);

    // Crop-edit: dim pixels outside the crop rectangle (Lightroom-style)
    if params.crop_edit != 0 {
        let inside = display_uv.x >= params.crop_left
            && display_uv.x <= params.crop_right
            && display_uv.y >= params.crop_top
            && display_uv.y <= params.crop_bottom;
        if !inside {
            *output = vec4(r * 0.28, g * 0.28, b * 0.28, 1.0);
            return;
        }
    }

    *output = vec4(r, g, b, 1.0);
}

// ---------------------------------------------------------------------------
// Histogram compute (256 bins × 4 channels: R, G, B, Luma)
// ---------------------------------------------------------------------------

/// Sample a 256×256 UV grid of the source (with develop applied) into histogram bins.
/// Host dispatches `(32, 32, 1)` workgroups of size 8×8.
#[spirv(compute(threads(8, 8, 1)))]
pub fn hist_cs(
    #[spirv(global_invocation_id)] id: spirv_std::glam::UVec3,
    #[spirv(descriptor_set = 0, binding = 0)] source: &Image2d,
    #[spirv(descriptor_set = 0, binding = 1)] sampler: &Sampler,
    #[spirv(uniform, descriptor_set = 0, binding = 2)] params: &DevelopGpuParams,
    #[spirv(storage_buffer, descriptor_set = 0, binding = 3)] bins: &mut [u32],
) {
    const SIZE: u32 = 256;
    let x = id.x;
    let y = id.y;
    if params.has_image == 0 || x >= SIZE || y >= SIZE {
        return;
    }

    let uv = vec2(
        (x as f32 + 0.5) / SIZE as f32,
        (y as f32 + 0.5) / SIZE as f32,
    );
    // Explicit LOD required in compute (implicit LOD is fragment-only)
    let sample: Vec4 = source.sample_by_lod(*sampler, uv, 0.0);
    let rgb = develop_pixel(vec3(sample.x, sample.y, sample.z), params);

    // Display-referred sRGB (true curve) → bins 0..255
    let r = linear_to_srgb_channel(rgb.x);
    let g = linear_to_srgb_channel(rgb.y);
    let b = linear_to_srgb_channel(rgb.z);
    let l = linear_to_srgb_channel(luminance(rgb));

    let ri = display_to_bin(r);
    let gi = display_to_bin(g);
    let bi = display_to_bin(b);
    let li = display_to_bin(l);

    // bins: [0..256)=R, [256..512)=G, [512..768)=B, [768..1024)=Luma
    // Scope Device = 5, Semantics None = 0
    unsafe {
        spirv_std::arch::atomic_i_add::<u32, 5, 0>(&mut bins[ri as usize], 1u32);
        spirv_std::arch::atomic_i_add::<u32, 5, 0>(&mut bins[256 + gi as usize], 1u32);
        spirv_std::arch::atomic_i_add::<u32, 5, 0>(&mut bins[512 + bi as usize], 1u32);
        spirv_std::arch::atomic_i_add::<u32, 5, 0>(&mut bins[768 + li as usize], 1u32);
    }
}
