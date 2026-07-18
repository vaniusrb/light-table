//! Image orientation from camera metadata (LibRaw flip / EXIF).
//!
//! Sensor buffers are usually stored landscape; portrait shots are flagged with
//! an orientation tag. We map that into non-destructive [`crate::crop::CropState`]
//! transforms (90° turns + flips) so the canvas matches other software.

/// Display orientation relative to the source pixel buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageOrientation {
    /// Quarter-turns **counter-clockwise** (0…3), same as `CropState::orient_90`.
    pub orient_90: u32,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl ImageOrientation {
    pub fn identity() -> Self {
        Self::default()
    }

    pub fn is_identity(self) -> bool {
        self.orient_90 % 4 == 0 && !self.flip_h && !self.flip_v
    }

    /// Apply to crop tool state (replaces rotation/flips; keeps crop rect / angle).
    pub fn apply_to_crop(self, crop: &mut crate::crop::CropState) {
        crop.orient_90 = self.orient_90 % 4;
        crop.flip_h = self.flip_h;
        crop.flip_v = self.flip_v;
    }

    /// LibRaw / dcraw `sizes.flip` (bitmask used by `flip_index`).
    ///
    /// Common values from EXIF:
    /// - 0: none
    /// - 3: 180°
    /// - 5: 90° CCW
    /// - 6: 90° CW
    ///
    /// Bits: 1 = mirror H, 2 = mirror V, 4 = transpose (swap axes).
    pub fn from_libraw_flip(flip: i32) -> Self {
        // Normalize degrees-style values some writers leave behind
        let flip = match (flip + 3600) % 360 {
            90 => 6,
            180 => 3,
            270 => 5,
            n if (0..=7).contains(&n) => n,
            _ => flip & 7,
        };
        let flip = flip & 7;

        // Prefer the documented discrete set, then bit decomposition.
        match flip {
            0 => Self::identity(),
            1 => Self {
                orient_90: 0,
                flip_h: true,
                flip_v: false,
            },
            2 => Self {
                orient_90: 0,
                flip_h: false,
                flip_v: true,
            },
            3 => Self {
                orient_90: 2, // 180°
                flip_h: false,
                flip_v: false,
            },
            // 5 = 90° CCW, 6 = 90° CW (LibRaw docs)
            5 => Self {
                orient_90: 1,
                flip_h: false,
                flip_v: false,
            },
            6 => Self {
                orient_90: 3, // 90° CW = 3× CCW
                flip_h: false,
                flip_v: false,
            },
            // 4 = transpose only; 7 = transpose + H + V
            4 => Self {
                // (x,y)→(y,x) ≈ 90° CW + flip H in y-down UV
                orient_90: 3,
                flip_h: true,
                flip_v: false,
            },
            7 => Self {
                orient_90: 1,
                flip_h: true,
                flip_v: false,
            },
            _ => Self::identity(),
        }
    }

    pub fn label(self) -> String {
        if self.is_identity() {
            return "0°".into();
        }
        let mut parts = Vec::new();
        match self.orient_90 % 4 {
            1 => parts.push("90° CCW"),
            2 => parts.push("180°"),
            3 => parts.push("90° CW"),
            _ => {}
        }
        if self.flip_h {
            parts.push("flip H");
        }
        if self.flip_v {
            parts.push("flip V");
        }
        if parts.is_empty() {
            "0°".into()
        } else {
            parts.join(" + ")
        }
    }
}
