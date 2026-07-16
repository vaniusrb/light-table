# Plan: better RAW → linear pipeline (LibRaw)

## Context

Today light-table opens RAW roughly as:

```text
LibRaw open → camera WB / matrix → unpack → dcraw_process (mostly defaults)
  → 8/16-bit RGB bitmap
  → assume “sRGB-ish” → srgb_to_linear()
  → GPU Rgba16Float (treated as linear)
```

The README **model limit** for RAW decode is **not** “LibRaw can only produce sRGB.”  
It is **how we use LibRaw** (and a thin `rsraw` API surface).

LibRaw can demosaic into several spaces and can emit **linear** RGB (`gamm = 1,1`). We barely configure it and then force an sRGB inverse on the result.

---

## Goals

1. **Honest working space** — after open, GPU pixels are linear in a **known** RGB space (or documented “LibRaw linear sRGB”).
2. **No double gamma** — do not call `srgb_to_linear` on data that is already linear.
3. **Stay on LibRaw for demosaic** — no custom CFA demosaic in v1 of this plan.
4. **Keep progressive open** — thumb → half-size proxy → full-res export still works.
5. **Extend vendored `rsraw`** only as needed (we already patch `third_party/rsraw` for Windows).

## Non-goals (this plan)

- Matching Adobe Lightroom color science
- Full ICC soft-proof / Display P3 pipeline
- GPU demosaic from mosaic + black level (possible later; separate project)
- Replacing LibRaw with another engine (optional alternative, not required)

---

## What LibRaw can do (relevant knobs)

| Knob | Role |
|---|---|
| `use_camera_wb` / `user_mul` | White balance |
| `use_camera_matrix` | Camera color matrix |
| `output_color` | Output space: raw RGB, sRGB, Adobe, Wide, ProPhoto, XYZ, ACES, … |
| `gamm[0]`, `gamm[1]` | Gamma; **linear ≈ `1.0, 1.0`** |
| `output_bps` | 8 / 16 (already set via `process::<BIT_DEPTH_*>`) |
| `half_size` | Fast proxy (already used) |
| `no_auto_bright` / `bright` | Avoid surprise stretch of the histogram |
| `highlight` | Highlight recovery mode |

`rsraw` today only exposes a subset (`half_size`, camera WB, camera matrix, bit depth). Full control means adding thin setters on `RawImage` that write `libraw_data_t.params`.

---

## Phased plan

### Phase 0 — Document current behavior (done in spirit)

- [x] README model limit: RAW treated as sRGB-ish after LibRaw process  
- [x] GPU/CPU display sRGB uses real IEC piecewise transfer (hist + present + export)

### Phase 1 — Easy win: linear LibRaw output

**Intent:** stop the wrong “decode as sRGB then linearize” path when we asked LibRaw for linear.

1. **Extend vendored `rsraw`** (`third_party/rsraw/rsraw/src/raw.rs`):
   - `set_gamma(g1: f32, g2: f32)` → `params.gamm[0]`, `params.gamm[1]`
   - `set_output_color(space: OutputColor)` → `params.output_color` (enum mapped to LibRaw constants)
   - `set_no_auto_bright(bool)` → `params.no_auto_bright`
   - Optional: `set_bright(f32)` if needed later

2. **Decode policy in `image_io::load_raw`:**
   - For **develop proxy + full-res**:
     - `set_gamma(1.0, 1.0)` (linear)
     - `set_output_color(Srgb)` *or* `Xyz` / `Adobe` (pick one default; document it)
     - `set_no_auto_bright(true)` so levels stay predictable
     - Keep camera WB + matrix as now
   - Map samples: `sample / max` as **linear** (no `srgb_to_linear`)
   - Clamp / soft-handle out-of-range if LibRaw still spikes

3. **Thumbnails (stage 1 progressive open):**
   - Embedded JPEG stays **display-referred** → keep `srgb_to_linear` on thumb decode only
   - Label quality `Thumbnail` remains “preview only”

4. **Tests / checks:**
   - Open a CR2/CR3: half-size path looks plausible; exposure ±1 EV behaves like linear
   - Histogram no longer assumes double-encoded data
   - Export still uses correct linear → sRGB encode

**Exit criteria:** GPU buffer is linear for demosaiced RAW (half + full); only JPEG thumbs use sRGB inverse.

### Phase 2 — Explicit working space

**Intent:** know *which* linear RGB we are in, not only “linear.”

1. Choose a **working space** for the app:
   - **Option A (simpler):** LibRaw linear **sRGB** primaries (matrix already baked by LibRaw)
   - **Option B (better headroom):** LibRaw linear **ProPhoto** or **XYZ** → convert once on CPU to a fixed working RGB (e.g. linear sRGB or ACEScg-like) before upload

2. Store on `DecodedImage` (or session metadata):
   - `color_space: WorkingSpace` enum
   - Optional: note “thumb is display-sRGB-derived”

3. Display / export:
   - Present + hist: working linear → display sRGB (matrix if needed + IEC transfer — already have transfer)
   - Export: same path as present for color

4. White balance:
   - Prefer applying WB in LibRaw (camera multipliers) while still linear
   - Later: user Temp/Tint as multipliers in working linear (closer to true)

**Exit criteria:** one documented working space; display/export transforms are explicit.

### Phase 3 — Better previews without full Adobe path

1. **Half-size linear** always for interactive develop (already half_size; ensure linear settings apply)
2. Optional **quarter-size** for huge files before half-size
3. **Histogram** sample in working linear then same display transform as canvas (already closer after Phase 1)
4. Avoid auto-bright so hist matches EV adjustments

### Phase 4 (optional / large) — Beyond LibRaw process

Only if Phase 1–2 are not enough:

| Approach | Pros | Cons |
|---|---|---|
| GPU demosaic from mosaic + metadata | Full control, pure Rust GPU story | Huge; needs CFA, black/white levels, scales |
| External profiles (DCP/ICC) | Better color | Complex; licensing/data |
| Different raw engine | Maybe better API | Another dependency; still need a policy |

---

## Suggested defaults (Phase 1)

| Setting | Value | Why |
|---|---|---|
| `half_size` | true for proxy | Speed |
| `use_camera_wb` | true | Sensible default |
| `use_camera_matrix` | true (as now) | Camera→RGB |
| `gamm` | `1.0, 1.0` | Linear samples |
| `output_color` | sRGB (LibRaw enum) | Familiar primaries; linear sRGB is still linear |
| `no_auto_bright` | true | Stable levels / histogram |
| Upload | `v / 65535` (or 255) **without** `srgb_to_linear` | Matches linear out |

Alternative: `output_color = XYZ` + small 3×3 to linear sRGB on CPU if we want a cleaner separation later (Phase 2B).

---

## Code touch points

| Area | Files |
|---|---|
| LibRaw params API | `third_party/rsraw/rsraw/src/raw.rs` (+ re-exports in `lib.rs`) |
| Decode / upload | `src/image_io.rs` (`load_raw`, `processed_*_to_decoded`) |
| Metadata on buffer | `DecodedImage` in `image_io.rs` |
| Docs | `README.md` model limits (update after Phase 1), this file |
| Display (already OK) | `shader` `linear_to_srgb_channel`, `save_srgb_image` |

---

## Risks

1. **Visual change** — linear LibRaw without auto-bright can look darker/flatter than today’s “pretty” process; exposure defaults may need a light rebalance.
2. **LibRaw space ≠ IEC sRGB** exactly even when `output_color = sRGB` (dcraw heritage).
3. **Thumb vs proxy mismatch** — progressive open will still jump from JPEG thumb to linear demosaic; status text already distinguishes quality.
4. **rsraw completeness** — some params need `unsafe` writes to `libraw_data_t`; keep changes minimal and documented.

---

## Verification checklist

- [ ] Build with `--features raw` / default features on Windows (`LIBCLANG_PATH`)
- [ ] CR2/CR3 open: thumb then half-size; no crash
- [ ] Exposure ±2 EV looks even (linear) on RAW proxy
- [ ] Histogram low end continuous (no regression of sRGB dip fix)
- [ ] Export JPEG matches on-screen develop for same params (same color policy)
- [ ] JPEG/PNG path unchanged (still sRGB → linear on load)

---

## Summary

| Question | Answer |
|---|---|
| Is the limit “because of LibRaw”? | **Mostly no** — LibRaw is more capable than we use. |
| Can we get around it? | **Yes** for linear + known space via LibRaw params + honest upload. |
| Match Lightroom? | **No** with process-only; that needs profiles / custom pipeline. |

**Recommended first implementation:** Phase 1 only (linear out + no erroneous `srgb_to_linear` on demosaiced RAW).
