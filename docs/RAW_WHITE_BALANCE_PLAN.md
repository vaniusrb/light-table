# Plan: RAW white balance, temperature & AWB

## Status

**Not implemented** (except baseline as-shot WB already in the linear pipeline).  
Related: [`RAW_LINEAR_PIPELINE_PLAN.md`](RAW_LINEAR_PIPELINE_PLAN.md), optional look: [`RAW_DEFAULT_LOOK_PLAN.md`](RAW_DEFAULT_LOOK_PLAN.md).

---

## Context

### What light-table does today

| Path | Behavior |
|---|---|
| **Bayer develop (GPU demosaic)** | LibRaw **`cam_mul`** (as-shot), green-normalized, highlight-safe soft WB, then **`rgb_cam`** → linear sRGB |
| **LibRaw process / export** | `use_camera_wb = true`, `use_camera_matrix = true` |
| **Develop Temp / Tint sliders** | Soft RGB gains on **already WB’d** linear sRGB (`±0.35` style), **not** Kelvin / CCT |
| **`pre_mul`** | Read into `SensorMeta`, **unused** |
| **Auto WB (scene analysis)** | **Not implemented** |

So:

- **As-shot / camera WB** — handled in a standard LibRaw way (good baseline).  
- **Temperature as physical CCT (Kelvin)** — **not** correct.  
- **AWB (recompute from image)** — **not** implemented.  
- UI **Temp = 0** means “leave as-shot,” not “5500 K.”

### Why this matters

Darktable / Lightroom typically expose:

1. **As shot** multipliers from the file  
2. **Camera / daylight / custom** modes  
3. **Temperature (K) + Tint** as real linear multipliers in camera RGB  
4. Optional **Auto** (gray-world, white patch, etc.)

Without (3)–(4), fine color matching and “set daylight 5600 K” workflows are impossible; only a mild warm/cool cast on top of baked as-shot WB.

---

## Goals

1. **Keep as-shot** as the default open WB (current behavior, refined).  
2. **True Temp (Kelvin) + Tint** as linear channel multipliers applied in **camera RGB** (before `rgb_cam`), not fake post-sRGB gains.  
3. Optional **Auto WB** mode (simple, documented algorithm).  
4. UI that shows meaningful units (K + tint) and which **WB source** is active.  
5. GPU demosaic + CPU twin + export (LibRaw path) stay **consistent** for the same mode.  
6. Do **not** break highlight recovery (magenta-safe whites).

## Non-goals

- Matching Adobe / DT pixel-perfect for every body  
- Dual-illuminant blend UI (can note as future)  
- Full spectral CCT models or skin-tone AWB ML  
- Replacing camera matrix with DCP (separate profiles work)

---

## Concepts (keep separate)

```text
  Mosaic / camera RGB
       │
       ├─ WB multipliers (this plan)
       │     source: as-shot | auto | manual K+tint | daylight pre_mul
       │
       ├─ Highlight-safe soft WB near clip (already done)
       │
       └─ rgb_cam → linear sRGB working space
              │
              └─ Develop (exposure, contrast, …)
                    └─ Temp/Tint today wrongly lives here → move / replace
```

| Term | Meaning |
|---|---|
| **`cam_mul`** | As-shot multipliers from camera/Exif (LibRaw) |
| **`pre_mul`** | Often daylight / reference multipliers |
| **As shot** | Use `cam_mul` (normalize green ≈ 1) |
| **Auto** | Estimate multipliers from demosaiced/camera data |
| **Manual** | User Temp (K) + Tint → multipliers |
| **Develop cast** | Current soft gains — deprecate as primary WB |

---

## Phase 0 — Document & inventory ✅ planning

1. [x] This plan file.  
2. [ ] README model limits: clarify as-shot only; Temp slider is not Kelvin.  
3. [ ] Log on open: `cam_mul`, `pre_mul`, and chosen WB mode (once modes exist).

---

## Phase 1 — WB source model & data plumbing

### 1a. Types

```text
enum WbSource {
  AsShot,      // cam_mul (default)
  Daylight,    // pre_mul when valid, else as-shot
  Auto,        // estimated (Phase 3)
  Manual,      // from temperature_k + tint
}

struct WhiteBalanceState {
  source: WbSource,
  /// Effective multipliers [R, G, B] (green-normalized), applied in camera RGB
  mul: [f32; 3],
  /// UI: Kelvin when Manual or when derived for display
  temperature_k: f32,  // e.g. 2000..12000
  tint: f32,           // green↔magenta, e.g. -150..150 or -1..1 (pick one scale)
}
```

Attach to session (app) and optionally to `MosaicBuffer` metadata for re-demosaic.

### 1b. Files

| Area | Touch |
|---|---|
| Metadata | `SensorMeta` already has `cam_mul`, `pre_mul` |
| State | `src/develop.rs` or new `src/wb.rs` |
| Demosaic uniforms | `DemosaicGpuParams` cam_mul_* already; update when source changes |
| Re-run demosaic | On WB change: re-dispatch GPU demosaic (or cheaper: store camera-RGB texture + WB pass — Phase 2 design choice) |

### Design choice: where to apply WB

| Option | Pros | Cons |
|---|---|---|
| **A. Bake WB into demosaic** (current) | One pass | Changing Temp requires full re-demosaic |
| **B. Demosaic → camera RGB texture, WB+matrix as 2nd pass** | Fast Temp/Tint scrubbing | Extra texture + pass |
| **C. Hybrid** | Open: bake as-shot; Manual: 2nd pass | More code |

**Recommendation:**  
- **Phase 1–2:** keep bake-in demosaic; on Manual/Auto change, re-run demosaic (acceptable for proxy sizes ≤ 3200).  
- **Later:** split camera-RGB intermediate if Temp scrubbing feels slow.

**Exit:** `WhiteBalanceState` exists; as-shot still default; multipliers computed in one place (`wb::multipliers_for(...)`).

---

## Phase 2 — True temperature (Kelvin) + tint → multipliers

### 2a. CCT → RGB gains (practical approach)

We do **not** need a full Planck locus implementation on day one.

**Pragmatic v1 (good enough):**

1. Treat **as-shot** as the neutral reference (mul = cam_mul).  
2. User Temp/Tint are **offsets from as-shot**, mapped to extra linear gains:  
   - Warmer (lower K than as-shot estimate): boost R, cut B  
   - Cooler: cut R, boost B  
   - Tint: G vs magenta (R+B)  

Or:

**Better v1:** invert / fit a simple model:

1. Estimate as-shot correlated color temperature from `cam_mul` (rough: ratio R/B → K via a small LUT or quadratic).  
2. UI shows that K as starting point when opening RAW.  
3. User moves K/tint → new multipliers via standard **temperature→RGB** approximation in camera space, then normalize G=1.

Reference approaches used by raw apps:

- Bradford / von Kries in XYZ (heavier)  
- Empirical: `mul_r(K)`, `mul_b(K)` tables  
- DT-style: illuminant → coeffs with camera matrix  

**Recommendation for light-table:**  

1. Open: `source = AsShot`, mul = normalized `cam_mul`.  
2. Estimate display Kelvin from R/B ratio for UI only.  
3. Manual mode:  
   ```text
   mul = normalize( cam_mul * delta_mul(temperature_k, tint) )
   ```  
   or absolute:  
   ```text
   mul = normalize( kelvin_tint_to_mul(K, tint) )
   ```  
   Prefer **absolute** once kelvin_tint_to_mul is stable; until then **offset from as-shot** is safer.

### 2b. Replace develop Temp/Tint

1. [ ] Remove (or hide) soft post-sRGB Temp/Tint from `develop_pixel` **when RAW uses new WB stack**.  
2. [ ] UI Temp slider: **Kelvin** (e.g. 2500–10000), Tint: separate scale.  
3. [ ] Changing sliders → `WbSource::Manual` → update mul → re-demosaic / re-apply WB.  
4. [ ] JPEG: keep soft Temp/Tint **or** disable (JPEG already baked WB).

### 2c. Highlight recovery

Keep soft_wb_mul / neutral blend **after** choosing final multipliers (already in demosaic). Re-test clipped lamps after Manual extreme Kelvin.

**Exit:** Moving Temp changes color like a real raw editor; units are Kelvin; as-shot restore button sets source back to AsShot.

---

## Phase 3 — Auto WB

### 3a. Simple algorithms (pick one for v1)

| Method | Notes |
|---|---|
| **Gray-world** | Mean R,G,B → scale to equal; fails on dominant color scenes |
| **White-patch** | Use bright near-neutral percentile; better for many photos |
| **Hybrid** | White-patch with gray-world fallback |

**Recommendation:** white-patch on a downscaled camera-RGB or post-demosaic sample grid (same 96² idea as base EV).

### 3b. UX

1. [ ] Button **Auto** → `WbSource::Auto`, store mul, update K estimate for sliders.  
2. [ ] Button **As shot** → restore cam_mul.  
3. [ ] Button **Daylight** → pre_mul if valid.  

**Exit:** Auto improves mixed indoor/outdoor mistakes for many files; documented failure cases (sunset, stage lighting).

---

## Phase 4 — LibRaw process / export parity

Interactive path may use GPU demosaic; export full uses LibRaw process.

1. [ ] When export uses LibRaw: set WB via `user_mul` / disable camera WB and push our effective multipliers.  
2. [ ] Or: export full via mosaic demosaic at full res (future) so one WB path only.  

**v1 recommendation:** map effective mul → LibRaw `user_mul` + `use_camera_wb = false` on full export so Manual/Auto match the canvas.

Need rsraw:

```text
set_user_mul(r, g, b, g2)
set_use_camera_wb(false)
```

**Exit:** Export JPEG matches on-screen WB for AsShot / Manual / Auto.

---

## Phase 5 — UX polish

1. [ ] Develop panel section **White balance**: source chips (As shot | Auto | Daylight | Manual).  
2. [ ] Kelvin + Tint sliders (disable or slave when As shot).  
3. [ ] Status: `WB as-shot` / `WB 5600 K tint +10` / `WB auto`.  
4. [ ] Reset develop: restore as-shot WB + default look (see default-look plan), not random soft gains.  
5. [ ] Optional: show raw `cam_mul` in a debug/log-only line.

---

## Suggested data flow (target)

```text
open RAW
  → unpack, cam_mul, pre_mul, rgb_cam
  → wb = AsShot(cam_mul)
  → demosaic with wb.mul + highlight recovery + matrix
  → develop (exposure, contrast, …)  // no second WB cast

user moves Kelvin
  → wb = Manual(K, tint) → mul = f(K, tint)   // or offset from as-shot
  → re-demosaic (or WB pass)
  → hist/present update

export full
  → LibRaw user_mul = wb.mul  (or GPU full demosaic later)
  → develop + crop
```

---

## Implementation order

| Step | Phase | Effort | Value |
|---|---|---|---|
| 1 | 0 + docs/README | Small | Clear expectations |
| 2 | 1 — `WhiteBalanceState`, single mul helper | Small | Foundation |
| 3 | 2 — Kelvin/tint → mul, wire demosaic, UI | Medium | Real Temp control |
| 4 | 4 — LibRaw user_mul export | Small–medium | Export matches |
| 5 | 3 — Auto WB | Medium | Convenience |
| 6 | 5 — polish | Small | Usability |
| 7 | Optional: camera-RGB intermediate for live scrubbing | Medium | Performance |

---

## Risks

1. **Wrong Kelvin model** — absolute CCT without camera calibration can drift; offset-from-as-shot is safer for v1.  
2. **Double WB** — if soft develop Temp remains while Manual mul applies → desaturated or weird color; remove post-WB cast for RAW.  
3. **Export mismatch** — LibRaw still on camera WB while UI is Manual → must set user_mul.  
4. **Re-demosaic cost** — full bilinear on huge sensors; stick to proxy size; debounce slider.  
5. **G vs G2** — rare sensors; keep averaging / max-green normalize as today unless issues appear.  
6. **Interaction with default look** — base EV and filmic-ish contrast stay independent of WB.

---

## Verification checklist

- [ ] Open RAW: as-shot matches current good baseline (no regression)  
- [ ] As shot → Manual: warmer K boosts R path, cooler boosts B; neutrals stay plausible  
- [ ] Extreme K still keeps clipped whites non-magenta (highlight recovery)  
- [ ] Auto on mixed daylight: less cast than wrong as-shot (where camera was wrong)  
- [ ] Auto on sunset: may fail gray-world — document  
- [ ] Export with Manual WB matches canvas  
- [ ] JPEG path unchanged (or soft Temp only)  
- [ ] Histogram / develop EV still coherent after WB change  

---

## Summary

| Question | Answer |
|---|---|
| Handling as-shot WB correctly today? | **Mostly yes** (`cam_mul` / LibRaw camera WB). |
| Handling temperature correctly? | **No** — UI Temp is a soft post-process cast, not Kelvin. |
| Handling AWB? | **No** — plan Phase 3. |
| First implementation? | WB state + true mul path + Kelvin/tint UI + export user_mul; then Auto. |

**Recommended first implementation:** Phase 1 + Phase 2 (as-shot default, Manual Kelvin/tint → camera-RGB multipliers, drop soft RAW Temp cast) + Phase 4 export parity. Auto WB next.
