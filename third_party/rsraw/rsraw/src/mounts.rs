use std::fmt::{self, Display};

use rsraw_sys as sys;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mounts(sys::LibRaw_camera_mounts);

impl Mounts {
    pub fn repr(&self) -> &'static str {
        match self.0 {
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Alpa => "Alpa",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_C => "C",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Canon_EF_M => "Canon EF-M",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Canon_EF_S => "Canon EF-S",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Canon_EF => "Canon EF",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Canon_RF => "Canon RF",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Contax_N => "Contax N",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Contax645 => "Contax645",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_FT => "FT",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_mFT => "mFT",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Fuji_GF => "Fuji GF",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Fuji_GX => "Fuji GX",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Fuji_X => "Fuji X",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Hasselblad_H => "Hasselblad H",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Hasselblad_V => "Hasselblad V",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Hasselblad_XCD => "Hasselblad XCD",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Leica_M => "Leica M",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Leica_R => "Leica R",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Leica_S => "Leica S",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Leica_SL => "Leica SL",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Leica_TL => "Leica TL",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_LPS_L => "LPS L",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Mamiya67 => "Mamiya67",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Mamiya645 => "Mamiya645",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Minolta_A => "Minolta A",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_CX => "Nikon CX",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_F => "Nikon F",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_Z => "Nikon Z",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_PhaseOne_iXM_MV => "PhaseOne iXM MV",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_PhaseOne_iXM_RS => "PhaseOne iXM RS",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_PhaseOne_iXM => "PhaseOne iXM",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Pentax_645 => "Pentax 645",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Pentax_K => "Pentax K",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Pentax_Q => "Pentax Q",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_RicohModule => "RicohModule",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Rollei_bayonet => "Rollei bayonet",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Samsung_NX_M => "Samsung NX M",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Samsung_NX => "Samsung NX",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Sigma_X3F => "Sigma X3F",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_Sony_E => "Sony E",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_LF => "LF",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_DigitalBack => "DigitalBack",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_FixedLens => "FixedLens",
            sys::LibRaw_camera_mounts_LIBRAW_MOUNT_IL_UM => "IL UM",
            _ => "Unknown",
        }
    }
}

impl AsRef<str> for Mounts {
    fn as_ref(&self) -> &str {
        self.repr()
    }
}

impl Display for Mounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.repr())
    }
}

impl From<sys::LibRaw_camera_mounts> for Mounts {
    fn from(mounts: sys::LibRaw_camera_mounts) -> Self {
        Mounts(mounts)
    }
}
