# Plan: better RAW → linear pipeline (LibRaw)

**Status: implementation complete through Phase 4c** (plus post-plan polish).  
Remaining items are **manual verification** and **optional future** work.

---

## Context (historical)

Originally light-table opened RAW roughly as:

```text
LibRaw open → camera WB / matrix → unpack → dcraw_process (mostly defaults)
  → 8/16-bit RGB bitmap
  → assume “sRGB-ish” → srgb_to_linear()
  → GPU Rgba16Float (treated as linear)
```

The README **model limit** for RAW decode is **not** “LibRaw can only produce sRGB.”  
It is **how we use LibRaw** (and a thin `rsraw` API surface).

LibRaw can demosaic into several spaces and can emit **linear** RGB (`gamm = 1,1`). We barely configured it and then forced an sRGB inverse on the result.

### Current pipeline (as implemented)

```text
RAW open (progressive):
  1. Embedded JPEG thumb  → display sRGB EOTF → linear sRGB  (EXIF orient baked when possible)
  2. [huge] quarter proxy → CPU half-Bayer + 2× box
  3. Develop proxy (Bayer):
       unpack mosaic → GPU demosaic (full bilinear if edge ≤ ~3200, else half 2×2)
       → black / cam_mul / rgb_cam → linear sRGB Rgba16Float
       → camera flip (LibRaw sizes.flip) via non-destructive crop orient
       → auto base Exposure EV (replaces LibRaw auto-bright visually)
  3'. Non-Bayer / fallback: LibRaw process linear XYZ → app XYZ→sRGB matrix
  4. Export full-res: LibRaw linear XYZ process (AHD-class); crop rect in display UV;
     file orient not double-applied (LibRaw already bakes flip)
```

JPEG/PNG: EXIF orientation baked into pixels; IEC sRGB EOTF → linear sRGB working space.

---

## Goals

| # | Goal | Result |
|---|---|---|
| 1 | Honest working space | ✅ **linear sRGB primaries** (`src/color.rs`) |
| 2 | No double gamma | ✅ linear RAW never run through `srgb_to_linear` |
| 3 | Stay on LibRaw for demosaic (v1) | ⚠️ **Superseded by Phase 4b/4c** for Bayer develop proxy; export still LibRaw |
| 4 | Keep progressive open | ✅ thumb → [quarter] → develop proxy → full on export |
| 5 | Extend vendored `rsraw` only as needed | ✅ gamma, output_color, no_auto_bright, SensorMeta, mosaic copy, flip |

---

## Non-goals (this plan) — still out of scope

- Matching Adobe Lightroom color science
- Full ICC soft-proof / Display P3 pipeline
- DCP / camera profiles
- Replacing LibRaw with another engine

GPU demosaic (Bayer half + full bilinear) was promoted from “later / separate” into **Phase 4b/4c** and **is implemented** for interactive develop.

---

## What LibRaw can do (relevant knobs)

| Knob | Role | Our use |
|---|---|---|
| `use_camera_wb` / `user_mul` | White balance | Camera WB on LibRaw process path; mosaic uses `cam_mul` |
| `use_camera_matrix` | Camera color matrix | Process path; mosaic uses `rgb_cam` |
| `output_color` | Output space | **XYZ** (4a process / export) |
| `gamm[0]`, `gamm[1]` | Gamma | **1,1** (linear) |
| `output_bps` | 8 / 16 | 8 proxy / 16 full |
| `half_size` | Fast proxy | LibRaw fallback path only (mosaic does its own half) |
| `no_auto_bright` | Avoid hist stretch | **true** (brightness via app base EV instead) |
| `sizes.flip` | Orientation | Applied on mosaic via crop; baked by LibRaw on process |

---

## Phased plan — completion status

### Summary

| Phase | Status | Notes |
|---|---|---|
| **0** Document behavior | ✅ done | README model limits + IEC display |
| **1** Linear LibRaw output | ✅ done | gamma 1,1; no false sRGB inverse |
| **2** Explicit working space (Option A) | ✅ done | linear sRGB; Option B deferred |
| **3** Better previews | ✅ done | progressive + edge cap + crop hist |
| **4a** Linear XYZ + SensorMeta | ✅ done | process / export path |
| **4b** GPU mosaic demosaic (half 2×2) | ✅ done | Bayer develop proxy |
| **4c** Full bilinear GPU demosaic | ✅ done | when edge ≤ budget |
| **Post** Base EV + orientation | ✅ done | polish after 4c |
| Manual verification checklist | ⬜ open | needs real CR2/CR3 smoke tests |
| Future: true linear Temp/Tint | ⬜ deferred | still parametric gains |
| Future: DCP/ICC / other engine | ⬜ deferred | — |

---

### Phase 0 — Document current behavior ✅

- [x] README model limits for RAW / color / develop  
- [x] GPU/CPU display sRGB uses real IEC piecewise transfer (hist + present + export)

---

### Phase 1 — Easy win: linear LibRaw output ✅

**Intent:** stop the wrong “decode as sRGB then linearize” path when we asked LibRaw for linear.

1. **Extend vendored `rsraw`** (`third_party/rsraw/rsraw/src/raw.rs`):
   - [x] `set_gamma(power, toe_slope)` via `libraw_set_gamma`
   - [x] `set_output_color(OutputColor)` → `params.output_color`
   - [x] `set_no_auto_bright(bool)` via `libraw_set_no_auto_bright`
   - [x] `OutputColor` enum re-exported from `rsraw`

2. **Decode policy** (evolved in 4a to XYZ; linear upload retained):
   - [x] Demosaic: gamma `(1,1)`, `no_auto_bright`
   - [x] Camera WB + matrix via LibRaw (process path)
   - [x] Upload / convert as **linear** — **no** `srgb_to_linear` on demosaiced RAW

3. **Thumbnails:**
   - [x] Embedded JPEG stays **display-referred** → `srgb_eotf` on thumb only

4. **Tests / checks:** (manual — still open)
   - [ ] Open a CR2/CR3: develop path looks plausible; exposure ±1 EV behaves like linear  
   - [ ] Histogram sensible on RAW proxy  
   - [ ] Export still uses correct linear → sRGB encode  

**Exit criteria:** ✅ GPU buffer is linear for demosaiced RAW; only JPEG thumbs use sRGB inverse.

---

### Phase 2 — Explicit working space ✅ (Option A)

**Intent:** know *which* linear RGB we are in, not only “linear.”

1. **Policy — Option A:** app-wide **linear sRGB primaries**
   - [x] RAW → linear sRGB (via XYZ matrix or mosaic `rgb_cam`)
   - [x] JPEG/PNG/thumbs: IEC sRGB EOTF → linear sRGB
   - [ ] **Option B** (ProPhoto / wider working space) — **deferred**

2. **Metadata on buffer:**
   - [x] `src/color.rs` — `WorkingSpace`, `SourceEncoding`, `ColorMeta`
   - [x] `DecodedImage.color` on every decode path
   - [x] Encodings include `LibRawLinearXyz`, `MosaicBayerLinear`, `DisplaySrgb`
   - [x] Shared CPU helpers `srgb_eotf` / `srgb_oetf` / `linear_srgb_to_u8`

3. **Display / export:**
   - [x] Present + hist: linear sRGB → IEC OETF  
   - [x] Export uses `color::linear_srgb_to_u8`  
   - [x] Status/log shows working space + source encoding on open  

4. **White balance:**
   - [x] Camera WB in LibRaw / `cam_mul` on mosaic  
   - [ ] User Temp/Tint as **true linear multipliers** — **deferred** (still parametric gains in develop)

**Exit criteria:** ✅ one documented working space; display/export transforms are explicit.

---

### Phase 3 — Better previews without full Adobe path ✅

1. **Interactive develop proxy**
   - [x] Half / full bilinear mosaic (4b/4c) or LibRaw half-size fallback  
2. **Quarter-size / GPU edge cap for huge files**
   - [x] Progressive: thumb → optional **quarter** when huge → develop proxy  
   - [x] `MAX_GPU_PROXY_EDGE` (3200) clamps interactive textures  
   - [x] Export still uses full-res LibRaw demosaic  
3. **Histogram aligned with canvas geometry + tone**
   - [x] Sample grid inside **crop** (display UV)  
   - [x] `display_to_source_uv` (rotate / 90° / flip) before texture fetch  
   - [x] `develop_pixel` then IEC sRGB bins (EV/WB match; NR/sharpen skipped)  
   - [x] Hist invalidates when crop/rotate/flip changes  
4. **Avoid auto-bright** so hist matches EV
   - [x] `no_auto_bright` on RAW demosaic; brightness via base Exposure EV (post-plan)

---

### Phase 4 — Beyond LibRaw process ✅ (4a–4c done)

| Approach | Status | Notes |
|---|---|---|
| **4a. Linear XYZ demosaic + app matrix → linear sRGB** | ✅ done | Process / export path |
| **4a. Sensor metadata API** | ✅ done | black, cam_mul, filters, mosaic, margins, **flip** |
| **4b. GPU demosaic from mosaic** | ✅ done | Bayer half-size 2×2; X-Trans → 4a fallback |
| **4c. Full-res bilinear GPU demosaic** | ✅ done | When mosaic edge ≤ GPU budget; else half 2×2 |
| External profiles (DCP/ICC) | ⬜ future | Better color; complex |
| Different raw engine | ⬜ future | Policy still required |

#### Phase 4a details ✅

1. Vendored `rsraw`: `SensorMeta` + `RawImage::sensor_meta()` after unpack  
2. Decode: `output_color = XYZ`, `gamm = 1,1`, `no_auto_bright`  
3. CPU convert: CIE XYZ (D65) → linear sRGB (`color::xyz_to_linear_srgb`)  
4. Tagging: `SourceEncoding::LibRawLinearXyz`  
5. Log: black level, cam_mul, CFA filters, mosaic presence  

#### Phase 4b details ✅

1. Vendored `rsraw`: `copy_mosaic_u16()` (active area), margins, `is_bayer()`  
2. Progressive open: thumb → optional quarter → **mosaic stage**  
3. GPU: mono `R32Float` → `fs_demosaic` half 2×2 + black / `cam_mul` / `rgb_cam` → `Rgba16Float`  
4. CPU twin: `demosaic::demosaic_bayer_half` for export proxy host pixels  
5. Tagging: `SourceEncoding::MosaicBayerLinear`  
6. Fallback: non-Bayer (X-Trans) or unpack failure → Phase 4a  
7. Export full-res: LibRaw linear XYZ process (not GPU demosaic)

#### Phase 4c details ✅

1. Mode 1: classic **bilinear** Bayer at full active resolution (`DemosaicMode::FullBilinear`)  
2. Selection: `MosaicBuffer::select_mode(MAX_GPU_PROXY_EDGE)`  
3. GPU + CPU twins share interpolate + WB + `rgb_cam` math  
4. Status/log reports demosaic mode  
5. Export full-res still LibRaw process  

---

### Post-plan polish ✅ (done after 4c)

These were not original phase checklists but shipped to fix real use issues:

| Item | What | Files |
|---|---|---|
| **Auto base Exposure EV** | Linear decode without auto-bright looks dark; estimate EV from ~99th-percentile luma → Exposure slider; Reset restores base | `src/color.rs` (`estimate_raw_base_exposure_ev`), `src/app.rs` |
| **Portrait / EXIF orientation** | Mosaic ignores sensor landscape layout; JPEG ignored EXIF | LibRaw `sizes.flip` → `ImageOrientation` → crop; JPEG bake via `image` decoder orientation | `src/orient.rs`, `rsraw` SensorMeta.flip, `image_io`, `app` export undoes double-rotate on LibRaw full |

---

## Suggested defaults (current)

| Setting | Value | Why |
|---|---|---|
| Progressive | thumb → [quarter] → mosaic/LibRaw proxy | Speed + UX |
| Bayer develop | GPU demosaic (bilinear or half 2×2) | Full control, no LibRaw process on interactive path |
| Non-Bayer / export full | LibRaw XYZ linear + app matrix | Quality demosaic on export |
| `gamm` | `1.0, 1.0` | Linear samples |
| `no_auto_bright` | true | Stable levels; base EV instead |
| Working space | linear sRGB | Option A |
| Base exposure | auto EV on RAW open | Match other apps’ brightness roughly |
| Orientation | flip / EXIF | Portrait upright |

---

## Code touch points (current)

| Area | Files |
|---|---|
| LibRaw params / mosaic API | `third_party/rsraw/rsraw/src/raw.rs` |
| Decode / progressive | `src/image_io.rs` |
| Color policy + base EV | `src/color.rs` |
| Bayer demosaic CPU | `src/demosaic.rs` |
| Orientation | `src/orient.rs` |
| GPU demosaic + textures | `src/gpu/pipelines.rs`, `src/gpu/textures.rs`, `shader/src/lib.rs` |
| App load / export | `src/app.rs` |
| Docs | `README.md`, this file |

---

## Risks (and mitigations)

1. **Dark linear RAW** — mitigated by **auto base Exposure EV** (`estimate_raw_base_exposure_ev`).  
2. **LibRaw / matrix ≠ IEC sRGB primaries exactly** — accepted for Option A.  
3. **Thumb vs proxy mismatch** — progressive jump still expected; status distinguishes quality; orient applied on mosaic stage.  
4. **rsraw completeness** — minimal unsafe param writes; documented.  
5. **GPU bilinear ≠ LibRaw AHD** — export full still uses LibRaw for quality.  
6. **Orientation export** — LibRaw full bakes flip; export subtracts `file_orientation` so crop orient is not double-applied.

---

## Verification checklist (manual — still open)

- [ ] Build with default features on Windows (`LIBCLANG_PATH`)  
- [ ] CR2/CR3 open: thumb then develop proxy; no crash  
- [ ] Portrait RAW upright (status shows orient)  
- [ ] Exposure base EV non-zero on dark linear RAW; ±2 EV behaves evenly  
- [ ] Histogram low end continuous (no regression of sRGB dip fix)  
- [ ] Export JPEG matches on-screen develop for same params  
- [ ] JPEG/PNG path: EXIF portrait upright; still sRGB → linear on load  

---

## Summary

| Question | Answer |
|---|---|
| Is the limit “because of LibRaw”? | **Mostly no** — we now use linear params, XYZ, and GPU mosaic demosaic. |
| Plan implementation complete? | **Yes through Phase 4c + post polish.** |
| What’s left? | Manual smoke tests; optional Temp/Tint linear gains, profiles, other engines. |
| Match Lightroom? | **No** — needs DCP/ICC / full tone science (non-goal). |

**Original recommended first step was Phase 1 only.** The full plan (0→4c) plus orientation and base-EV polish has since been implemented.
