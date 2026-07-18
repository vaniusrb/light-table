# Plan: Darktable-like default look for RAW

## Status

**Not implemented** — planning only.  
Depends on the completed linear RAW pipeline: [`RAW_LINEAR_PIPELINE_PLAN.md`](RAW_LINEAR_PIPELINE_PLAN.md).

---

## Context

After the linear pipeline + highlight recovery + base EV, light-table’s RAW open is close to **Windows Photos**: honest linear-ish color, modest brightness, relatively flat contrast.

**Darktable** (and Lightroom, Capture One, etc.) usually show a **finished default edit**, not pure linear sensor data. That is why the same CR2/CR3 often looks **a little brighter and more contrasty** there.

This plan adds an optional, explicit **“RAW default look”** so light-table can open closer to Darktable-style defaults, without implementing Darktable’s full module graph or Adobe profiles.

---

## What Darktable does (relevant subset)

On a new RAW, Darktable typically auto-enables a **pixel workflow** (user preference):

| Layer | Darktable (modern scene-referred default) | Older / alt path |
|---|---|---|
| Sensor | Black/white, demosaic, WB | Same |
| Color in | Input profile (camera matrix / base profile) | Same family |
| Exposure | Often slight auto exposure | Manual |
| Tone | **Filmic RGB** or **Sigmoid** (S-shaped, midtone contrast, highlight roll-off) | **Base curve** (maker / model JPEG-like curves) |
| Finish | Optional sharpen, color modules | Base-curve era presets |

Important distinctions:

1. Defaults are **modules with parameters**, not a secret second decode.  
2. Base curves can be **per manufacturer / model** (Exif make).  
3. Filmic/sigmoid are **generic** scene→display mappings (strong “look” without per-camera curves).  
4. User can change the default workflow in preferences (`auto-apply pixel workflow defaults`).

We will **not** port filmic’s full UI or DT’s profile database. We will **imitate the effect** with a small, documented default stack on top of our linear working space.

---

## Goals

1. **Optional RAW open look** that is brighter / more contrasty than pure linear (Darktable-ish “first impression”).  
2. **User-visible and editable** — defaults become starting `DevelopParams` (and maybe one new module), not baked into the texture.  
3. **Reset restores the same default look** for that open (not flat zero), with a clear way to get true linear (0 EV / no curve).  
4. **JPEG/PNG unchanged** by default (already display-referred; no filmic stack unless user opts in later).  
5. **Export matches canvas** for the same params.  
6. **Preference** to choose workflow: `Linear` (current) vs `Scene look` (DT-like) vs later `Base curve`.

## Non-goals

- Full Darktable filmic RGB / sigmoid parameter parity  
- Per-camera ICC/DCP/Adobe profiles  
- Porting DT’s base-curve database wholesale  
- Matching Lightroom Classic “Adobe Color”  
- Auto-applied denoise / local contrast / color calibration as defaults (can be later)

---

## Design: two layers of “default”

Keep **decode** and **look** separate (same as DT modules after demosaic).

```text
  [Decode — already done]
    mosaic/LibRaw → linear sRGB working texture
    orientation, highlight-safe WB, matrix

  [Default look — this plan]
    A. Base exposure EV     (already partially done)
    B. Tone mapping “look”  (new or stronger contrast curve)
    C. Mild midtone contrast / optional slight sat
    → DevelopParams (+ optional filmic-lite uniforms)

  [User develop]
    existing sliders adjust from that starting point
```

### Preference: `RawWorkflow`

```text
enum RawWorkflow {
  Linear,      // current: base EV only, flat contrast
  SceneLook,   // recommended DT-like: base EV + filmic-lite / S-curve
  // BaseCurve, // phase 3 optional: maker-inspired curve table
}
```

Default for new installs: **`SceneLook`** if we want “like other raw editors”; or **`Linear`** if we want to preserve current Windows-like behavior.  
**Recommendation:** ship `SceneLook` as default for RAW only; keep Linear available in UI/settings.

---

## Phase 0 — Document & preference hook ✅ small

1. [ ] This plan file (done when committed).  
2. [ ] README model limits: note “optional Darktable-like default look (not linear truth)”.  
3. [ ] App setting `RawWorkflow` (in-memory first; optional `serde` to a small config file later).  
4. [ ] Status line shows workflow, e.g. `look=scene` vs `look=linear`.

**Exit:** user can switch workflow without a rebuild (UI toggle or settings row).

---

## Phase 1 — Formalize base exposure as part of the default stack ✅ mostly exists

Today: `color::estimate_raw_base_exposure_ev` on RAW open.

1. [ ] Name it clearly as **default look layer A** (docs + log).  
2. [ ] Tune targets per workflow:
   - `Linear`: current ~99th pct → 0.78 (or slightly lower)  
   - `SceneLook`: slightly brighter mid target (e.g. 99th → **0.85–0.90**, or +0.15 EV bias on top of estimate)  
3. [ ] Store `raw_base_exposure` (already) and restore on Reset for RAW.  
4. [ ] UI: Reset develop = “reset to default look”, not necessarily all zeros; optional **“Linear zero”** control (set exposure 0 + disable look curve).

**Exit:** SceneLook opens a bit brighter than Linear on the same file; both editable.

---

## Phase 2 — Filmic-lite / scene-referred tone (core of DT-like contrast) 🎯 main work

Darktable filmic/sigmoid ≈ map scene linear → display with:

- Midtone contrast  
- Soft highlight roll-off (less hard clip)  
- Optional shadow lift  

### 2a. Minimal viable: parametric S-curve in linear light

Reuse/extend existing develop chain **before** IEC OETF:

| Param | Role | SceneLook default (starting guess) |
|---|---|---|
| `contrast` | Mid pivot contrast | **+0.15 … +0.25** |
| `shadows` | Lift darks slightly | **+0.05 … +0.10** |
| `highlights` | Soften brights | **−0.05 … −0.15** |
| `blacks` / `whites` | Endpoint | small or 0 |

Implementation:

1. [ ] `DevelopParams::default_for_raw(workflow)` / `default_for_raster()`.  
2. [ ] On RAW open after base EV: apply SceneLook defaults to contrast/shadows/highlights (not only exposure).  
3. [ ] Ensure GPU `develop_pixel` + CPU export twin stay in lockstep (already shared structure).  
4. [ ] Histogram uses same develop path (already) so peaks match the punchier look.

**Exit:** Same RAW looks closer to DT first open (brighter mids, more “pop”) without new shaders.

### 2b. Better: dedicated **filmic-lite** module (optional if 2a is too coarse)

If simple contrast params cannot match “DT feel” without wrecking colors:

1. [ ] Add `filmic_enabled`, `filmic_strength` (or fixed curve strength 0..1) to `DevelopGpuParams`.  
2. [ ] Shader (and CPU twin):  
   - Working luminance → log or power midtone curve → soft shoulder  
   - Re-apply chroma (preserve hue) so we don’t get grey mud  
3. [ ] SceneLook: `filmic_strength ≈ 0.5–0.7` by default; Linear: 0.  
4. [ ] UI slider under Develop: “Tone look” / “Filmic”.

**Exit:** One control approximates DT filmic “default contrast” better than raw contrast slider alone.

**Recommendation:** implement **2a first**; only do 2b if visual gap remains large.

---

## Phase 3 — Optional base-curve path (maker look) ⬜ later

Closest to old Darktable “base curve auto by Exif make”:

1. [ ] Read make/model from RAW (already via `rsraw` full_info / logs).  
2. [ ] Small **baked tables** (not full DT database): e.g. generic Canon / Nikon / Sony / Fuji / default S-curve as 1D LUT (256 or 1024 samples) in linear or after simple log.  
3. [ ] `RawWorkflow::BaseCurve` applies LUT + mild EV.  
4. [ ] Document: “inspired by maker curves, not a DT dump.”

**Non-goal:** pixel-match DT’s basecurve presets.

**Exit:** Optional third workflow for users who prefer JPEG-like maker contrast.

---

## Phase 4 — UX & product rules

1. [ ] **Open RAW**  
   - Apply `default_for_raw(workflow)` + base EV.  
   - Status: `base +1.2 EV · contrast +0.2 · look=scene`.  
2. [ ] **Open JPEG**  
   - `DevelopParams::default()` (zeros); no filmic stack.  
3. [ ] **Reset**  
   - Restore open defaults for that file type (RAW → scene look again).  
4. [ ] **“Neutral / linear” button**  
   - Exposure 0, contrast 0, filmic 0 — true flat linear for technical work.  
5. [ ] **Workflow preference**  
   - Toolbar or Develop panel: Linear | Scene | (Base curve).  
   - Changing workflow re-applies defaults (confirm if user has heavy edits).  
6. [ ] **Sidecar / future catalog**  
   - Store workflow + full DevelopParams so reopen is stable.

---

## Suggested default values (SceneLook v1)

Tune after visual A/B on a few CR2/CR3 vs Darktable 4.x scene-referred:

| Control | Linear (current) | SceneLook v1 (proposal) |
|---|---|---|
| Base EV | auto → ~0.78 p99 | auto → ~0.88 p99 **or** auto + **+0.2 EV** |
| Contrast | 0 | **+0.20** |
| Shadows | 0 | **+0.08** |
| Highlights | 0 | **−0.10** |
| Whites / Blacks | 0 | 0 |
| Vibrance / Sat | 0 | 0 (or vibrance **+0.05** only if needed) |
| Filmic-lite | off | off until Phase 2b |
| Sharpen | 0 | 0 (DT often sharpens; keep off to avoid noise surprise) |

These are **starting points**, not sacred numbers. Adjust in one place (`DevelopParams::default_for_raw`).

---

## Code touch points

| Area | Files |
|---|---|
| Defaults / workflow enum | `src/develop.rs` (new `RawWorkflow`, `default_for_raw`) |
| Base EV | `src/color.rs` (`estimate_raw_base_exposure_ev` targets) |
| Apply on open / reset | `src/app.rs` |
| UI toggle + reset semantics | `src/ui.rs` |
| Tone (2a) | existing `develop_pixel` (shader + CPU) |
| Tone (2b) | `shader/src/lib.rs`, `DevelopGpuParams` layout |
| Docs | this file, `README.md` model limits |

---

## Risks

1. **“Wrong” vs Windows** — SceneLook will diverge from Windows Photos (by design). Keep Linear workflow.  
2. **Double brightening** — base EV + contrast mid lift can blow faces; tune together; clamp auto EV max.  
3. **Hue shifts** — naive contrast in RGB can cast; prefer luminance-pivot contrast (we already pivot ~0.18) or filmic-lite with chroma preserve.  
4. **Export mismatch** — any new filmic param must exist on CPU export path.  
5. **Scope creep** — do not import DT’s module graph; stop at “default look ≈ DT first open.”

---

## Verification checklist

- [ ] Same portrait RAW: Linear ≈ current / Windows-ish  
- [ ] SceneLook: closer to Darktable scene-referred first open (brighter mids, more contrast)  
- [ ] Clipped whites still neutral (highlight recovery not broken)  
- [ ] Histogram matches punchier display  
- [ ] Export JPEG matches canvas for SceneLook  
- [ ] JPEG open still neutral defaults  
- [ ] Reset restores SceneLook defaults; Neutral forces linear zeros  
- [ ] Orientation + progressive open still correct  

---

## Implementation order (recommended)

| Step | Phase | Effort | Impact |
|---|---|---|---|
| 1 | 0 + 1 | Small | Workflow flag + brighter base EV for Scene |
| 2 | 2a | Small–medium | Default contrast/shadows/highlights — biggest perceived “DT-like” win |
| 3 | 4 | Small | UI + reset semantics |
| 4 | 2b | Medium | Filmic-lite only if 2a insufficient |
| 5 | 3 | Large | Maker base curves (optional) |

---

## Summary

| Question | Answer |
|---|---|
| Does Darktable use default “profile-like” settings? | **Yes** — workflow modules (filmic/sigmoid or base curve) + color in; not pure linear. |
| Can light-table match that feel? | **Mostly yes** with default Exposure + contrast stack (and optional filmic-lite). |
| Will we match Darktable pixel-perfect? | **No** — approximate “first open look.” |
| Keep current Windows-like view? | **Yes** via `RawWorkflow::Linear`. |

**Recommended first implementation:** Phase 0 + 1 + 2a + Phase 4 UX (workflow toggle, defaults on open, reset semantics). Defer filmic-lite and maker base curves until after visual review.
