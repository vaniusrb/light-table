# light-table

GPU-resident photo develop app (Lightroom-style basics), written in **Rust** end to end on the GPU path.

### Pure Rust on the GPU

All GPU work is authored in **Rust**, not GLSL, HLSL, or WGSL:

| Layer | Pure Rust? | Notes |
|---|---|---|
| **Shaders** (compute + fragment/vertex) | **Yes** | `shader/` crate via [rust-gpu](https://github.com/Rust-GPU/rust-gpu) → SPIR-V at build time |
| **GPU runtime / host** | **Yes** | [wgpu](https://wgpu.rs/) (Rust API over Vulkan) |
| **UI** | **Yes** | egui + egui-wgpu |
| **App / develop / crop math (CPU)** | **Yes** | Standard Rust |
| RAW decode (optional feature) | Host only | LibRaw via vendored `rsraw` (C++) — **not** on the GPU hot path |

There are **no hand-written shader languages** in this repo: present, denoise, sharpen, histogram, crop/rotate sampling are all `#![no_std]` Rust in `shader/src/lib.rs`, compiled with `spirv-builder`.

### Highlights

- **Working pixels stay on the GPU** after open (`Rgba16Float` source texture)
- **Non-destructive** develop, crop, and rotate (source texture is never overwritten)
- **Parametric pipeline** every frame in rust-gpu (fragment + compute)
- **egui** UI: histogram, sliders, crop/rotate tool, open/export
- **RAW** via vendored [rsraw](https://github.com/Hexilee/rsraw) (LibRaw) — CR2, CR3, NEF, ARW, DNG, …

---

## Requirements

| Component | Notes |
|---|---|
| Rust nightly | Pinned in `rust-toolchain.toml` (`nightly-2025-06-30`) — **required by rust-gpu** for pure-Rust shaders |
| Vulkan GPU | wgpu uses `Backends::VULKAN` (SPIR-V passthrough) |
| C++ toolchain | MSVC (Windows) / Clang / GCC — **only** for vendored LibRaw (`raw` feature), not for shaders |
| LLVM / libclang | **Windows:** set `LIBCLANG_PATH` for LibRaw bindgen (e.g. `C:\Program Files\LLVM\bin`) |
| `rust-src`, `rustc-dev`, `llvm-tools` | From the toolchain file (rust-gpu shader compile) |

---

## Build & run

```powershell
cd light-table

# Windows: required for RAW (bindgen → LibRaw headers)
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"

cargo run --release
```

First build is **slow**: rust-gpu compiles the SPIR-V codegen backend and your Rust shaders to `.spv`, and LibRaw (if enabled) builds as C++.

Optional logging:

```powershell
$env:RUST_LOG = "info"
cargo run --release
```

### Features

| Feature | Default | Description |
|---|---|---|
| `raw` | **on** | LibRaw via vendored `third_party/rsraw` (Windows static build fixes) |

Skip LibRaw compile:

```powershell
cargo run --release --no-default-features
```

---

## Supported formats

| Kind | Formats | Decoder |
|---|---|---|
| RAW | CR2, CR3, NEF, ARW, DNG, RAF, RW2, ORF, … | LibRaw (`rsraw`) |
| Raster | JPEG, PNG, TIFF, WebP, BMP, … | `image` crate |

### Progressive RAW open

Opening a RAW does **not** block on full demosaic:

1. **Embedded JPEG** preview (near-instant)
2. **Half-size demosaic** (8-bit LibRaw) for interactive develop
3. **Full-res** demosaic only on **export** (when needed)

---

## Usage

### Canvas & files

| Action | How |
|---|---|
| Open | **Open…** or `O` |
| Export | **Export…** — defaults to **JPEG** (PNG optional); develop + crop + rotate |
| Pan | Drag on the image (disabled in crop-edit) |
| Zoom | Mouse wheel (crop is the viewport; zoom-out does not show uncropped pixels) |
| Fit | **Fit** or `F` |
| Reset develop | Toolbar **Reset** (sliders only; not crop/rotate) |

### Develop (right panel)

| Section | Controls |
|---|---|
| **Histogram** | RGB + luma (GPU compute, small readback) |
| **Crop & rotate** | Edit crop, aspect, straighten, 90°, flips |
| **Light** | Exposure, contrast, highlights, shadows, whites, blacks |
| **White balance** | Temp, tint |
| **Presence** | Vibrance, saturation |
| **Detail** | Sharpen / radius / masking, luminance NR, color NR |

Each slider has **↻** (reset that control). Section headers have **↻** for the whole group.

### Crop & rotate (Lightroom-style)

Non-destructive; stored as parameters, applied at present + export.

| Action | How |
|---|---|
| Enter / leave crop | Toolbar **Crop** / **Done crop**, panel **Edit crop**, or `R` |
| Adjust crop | Drag edges, corners, or move the box |
| Grid | Rule-of-thirds; denser **6×6 + center cross** when straighten ≠ 0 |
| Aspect | Free, Original, 1:1, 4:3, 3:2, 16:9 |
| Straighten | **−45°…45°** — image rotates under the crop |
| 90° | **⟲ 90°** / **⟳ 90°** |
| Flip | **Flip H** / **Flip V** |
| Reset geometry | **Reset crop** (full frame, 0°, no flips) |

The photo is drawn only in the **central content area** (not under the right panel). Crop is the final viewport after you leave edit mode.

### Detail (GPU filters)

| Control | Role |
|---|---|
| **Sharpen** | Unsharp mask amount (0 = off) |
| **Radius** | Unsharp blur size |
| **Masking** | Soft threshold (less noise boost in flat areas) |
| **Luminance NR** | Edge-aware bilateral on luma |
| **Color NR** | Wider chroma blur |

Pipeline order: **denoise → sharpen → tone/WB/sat** (linear light), then sRGB for display.

---

## Architecture

### GPU path = pure Rust

```
  BUILD TIME                          RUNTIME (Rust host + SPIR-V)
  ─────────                           ────────────────────────────
  shader/src/lib.rs                   src/*  (wgpu, egui, decode)
  (Rust #![no_std])                         │
         │                                  │ write_texture (once per open)
         ▼                                  ▼
  spirv-builder ──► shader.spv        GPU texture Rgba16Float
  (rust-gpu)              │                 │
                          └──► wgpu loads SPIR-V (passthrough)
                                            │
                     ┌──────────────────────┼──────────────────────┐
                     ▼                      ▼                      ▼
              fs_present (Rust)      hist_cs (Rust)           egui overlay
              develop / crop /       atomic bins              (Rust)
              rotate / NR / sharp
```

No `.glsl` / `.wgsl` / `.hlsl` sources. CPU `#[repr(C)]` + `bytemuck` layouts match the shader crate.

```
Open (CPU, progressive for RAW)
  image / rsraw → linear f32 RGBA
       │
       ▼
  GPU texture (Rgba16Float)     ← pixel source of truth
       │
       ├─ fs_present (Rust → SPIR-V)
       │    content viewport → pan/zoom
       │    crop as viewport
       │    rotate / orient / flip (sample UV)
       │    denoise → sharpen → develop → sRGB
       ├─ hist_cs (Rust → SPIR-V) → 4 KB bin readback
       └─ egui (panels + crop grid overlay)
```

Hot path: tiny uniform buffer (+ optional histogram readback). No full-frame CPU pixel loop while editing.

| Path | Role |
|---|---|
| `src/app.rs` | Event loop, load channel, export, frame |
| `src/gpu/` | wgpu device, pipelines, texture upload (Rust) |
| `src/develop.rs` | Develop + GPU uniform layout, content viewport |
| `src/crop.rs` | Non-destructive crop / rotate / flip, export rasterize |
| `src/image_io.rs` | JPEG/PNG + progressive RAW (CPU cold path) |
| `src/ui.rs` | egui panels, histogram draw, crop overlay |
| `shader/src/lib.rs` | **All GPU kernels in Rust:** `vs_present`, `fs_present`, `hist_cs` |
| `third_party/rsraw/` | Vendored LibRaw (CPU RAW only; not GPU) |
| `build.rs` | `spirv-builder` compiles the shader crate → `shader.spv` |

More detail: [`ARCHITECTURE.md`](ARCHITECTURE.md).

---

## Keyboard

| Key | Action |
|---|---|
| `O` | Open file |
| `F` | Fit view |
| `R` | Toggle crop edit mode |
| `Esc` | Leave crop edit mode |

---

## Notes & limits

### Engineering

- **GPU shaders are pure Rust** (rust-gpu); only optional RAW decode pulls in C++ (LibRaw)
- **sRGB transfer** uses the IEC piecewise curve (`powf`) on GPU present + histogram and on CPU export; load uses the matching inverse
- **Vulkan validation** is off unless `WGPU_VALIDATION` is set (avoids missing-layer spam)
- **Export** re-applies develop on the CPU and rasterizes crop/rotate; full-res RAW demosaic can take a while

### Model limits (not full Lightroom)

These are intentional simplifications of the imaging model, not temporary UI gaps:

| Area | Current behavior | Limitation |
|---|---|---|
| **Color management** | Work in linear RGB; display/export via approximate **sRGB** | No ICC profiles, no Display P3 / Adobe RGB, no calibrated monitor path |
| **RAW decode** | LibRaw process → treat RGB as **sRGB-ish** then convert to linear | Mostly **our** thin use of LibRaw (not a hard LibRaw ceiling). Plan to improve: [`docs/RAW_LINEAR_PIPELINE_PLAN.md`](docs/RAW_LINEAR_PIPELINE_PLAN.md) |
| **Develop ops** | Parametric exposure, contrast, shadows/highlights, WB gains, vibrance/sat | Simple curves/gains — not Lightroom’s full tone curve, HSL, calibration, or profile-based rendering |
| **Denoise / sharpen** | 5×5 bilateral NR + unsharp mask in linear light | Local only; no multi-scale NR, no masking maps, no detail vs smoothing split like LR |
| **Crop / rotate** | Non-destructive UV crop, straighten, 90°, flips | No perspective / keystone, no guided upright, no soft-proof crop for print |
| **Histogram** | 256² sample grid + tone; bins in display sRGB | Does not fully mirror crop/rotate/spatial filters; not a pixel-perfect full-frame hist |
| **Working format** | GPU `Rgba16Float` after open | Good for interactive preview; not a 32-bit or camera-raw working space for print-critical work |
| **Catalog / library** | Single-image session | No filmstrip database, ratings, sync, or batch |

### Possible improvements

Rough priority ideas for later work (not a commitment):

1. **Color**
   - Shared CPU/GPU color helpers so encode/decode cannot drift
   - Optional working space + ICC-aware export / soft proof
   - Better RAW path (camera WB multipliers, matrix, optional scene-linear before tone)

2. **Develop**
   - Editable tone curve, HSL / color mixer, clarity / dehaze
   - Graduated / radial / brush masks (extra GPU passes)
   - Lens corrections (distortion, vignetting, CA) if metadata allows

3. **Detail**
   - Multi-pass / dual-res denoise; luminance vs color detail controls
   - Output sharpening vs capture sharpening
   - Histogram that applies the same crop/rotate/viewport as the canvas

4. **Geometry & UX**
   - Perspective transform; straighten via drag on a guide line
   - Dual-res preview (fast proxy always; full-res when zoomed 1:1)
   - Async full-res refine after half-size RAW open

5. **Library & export**
   - Sidecar develop settings (JSON / XMP-like)
   - Batch export, quality presets, embedded color profile in JPEG
   - Simple on-disk catalog / filmstrip

6. **Platform**
   - Broader backends (or documented DX12/Metal path) if Vulkan-only is a problem
   - Optional WGSL fallback for machines without rust-gpu toolchain

---

## License

App code: use as you prefer for this local project.

LibRaw (via `rsraw` / `third_party/rsraw`) has its **own** terms — see [LibRaw licensing](https://www.libraw.org/) before redistribution.
