# light-table architecture

## Pure Rust GPU

GPU operations are written in **Rust only**:

- **Shaders:** `shader/src/lib.rs` (`#![no_std]` Rust) → [rust-gpu](https://github.com/Rust-GPU/rust-gpu) / `spirv-builder` → SPIR-V
- **Runtime:** [wgpu](https://wgpu.rs/) (Rust), SPIR-V passthrough on Vulkan
- **No** project-authored GLSL, WGSL, or HLSL

CPU cold path may use LibRaw (C++) for RAW decode only; that never runs on the GPU hot path.

## Stack

| Layer | Tech |
|---|---|
| GPU runtime | wgpu 0.19 (Vulkan, SPIR-V passthrough) — **Rust** |
| Shaders | **Rust** via rust-gpu → SPIR-V (`shader/` crate) |
| UI | egui 0.27 + egui-wgpu + egui-winit — **Rust** |
| Window | winit 0.29 — **Rust** |
| Raster I/O | `image` — **Rust** |
| RAW I/O | vendored `third_party/rsraw` (LibRaw C++; CPU only) |

`rust-toolchain.toml` and the `spirv-builder` / `spirv-std` git `rev` must stay in sync: rust-gpu tracks a specific Rust nightly.

## Data residency

1. **Cold open:** CPU decodes to linear float RGBA (progressive for RAW).
2. **Upload:** `queue.write_texture` → `Rgba16Float` source texture.
3. **Hot path:** `DevelopGpuParams` uniform (+ optional ~4 KB histogram readback).
4. **Source texture is never modified.** Crop, rotate, and develop are parameters applied when sampling / presenting.

## Present path (`fs_present`)

1. Map screen UV into **content viewport** (central UI area; not under side panel).
2. Letterbox / pan / zoom; **crop is the viewport** outside crop-edit mode.
3. Map display UV → source UV: inverse straighten, 90° orient, flips.
4. Denoise (bilateral) → sharpen (unsharp) → tone / WB / presence.
5. Linear → approximate sRGB; crop-edit dims outside crop.

## Shader entry points

| Entry | Type | Role |
|---|---|---|
| `vs_present` | vertex | Fullscreen triangle |
| `fs_present` | fragment | Viewport, crop, rotate, develop, display |
| `hist_cs` | compute | 256² samples → 1024 atomic bins (R/G/B/Luma) |

## Geometry (non-destructive)

| State | Stored as | Applied |
|---|---|---|
| Crop rect | UV 0..1 on oriented image | Present + export |
| Straighten | degrees (−45…45) | Inverse rotate on sample UV |
| 90° / flip | `orient_90`, `flip_h/v` | Inverse on sample UV |

Export uses `crop::render_crop_rotate` to bake geometry after CPU develop.

## RAW path

```text
open bytes → optional embedded JPEG thumb
          → half_size + 8-bit process (interactive)
          → full-res 16-bit process (export only)
```

Windows: `LIBCLANG_PATH` for bindgen; vendored `rsraw-sys` build defines `LIBRAW_NODLL` (static LibRaw).
