use std::borrow::Cow;

use chrono::{DateTime, Local, TimeZone};
use rsraw_sys as sys;

use crate::{
    err::{Error, Result},
    processed::ProcessedImage,
    GpsInfo, LensInfo, ThumbnailImage, Thumbnails,
};

pub type BitDepth = u32;

pub const BIT_DEPTH_8: BitDepth = 8;
pub const BIT_DEPTH_16: BitDepth = 16;

/// LibRaw `params.output_color` (`-o` flag).
#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutputColor {
    /// Camera raw RGB (unique per camera)
    Raw = 0,
    /// sRGB D65 (default in LibRaw)
    Srgb = 1,
    Adobe = 2,
    WideGamut = 3,
    ProPhoto = 4,
    Xyz = 5,
    Aces = 6,
}

/// Sensor / color metadata from LibRaw (for linear pipelines & GPU demosaic).
#[derive(Clone, Debug)]
pub struct SensorMeta {
    pub black: u32,
    pub data_maximum: u32,
    pub maximum: u32,
    /// Camera white-balance multipliers [R,G,B,G2].
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    /// Camera RGB → reference RGB matrix (3×3, rows).
    pub rgb_cam: [[f32; 3]; 3],
    /// CFA filter pattern bitfield (`idata.filters`).
    /// Bayer: non-zero and not `!0`. X-Trans uses `0` with a separate 6×6 table.
    pub filters: u32,
    pub colors: i32,
    pub raw_width: u32,
    pub raw_height: u32,
    /// Visible / active area (after margins).
    pub width: u32,
    pub height: u32,
    pub left_margin: u32,
    pub top_margin: u32,
    /// Bytes per raw row (`sizes.raw_pitch`).
    pub raw_pitch: u32,
    /// True if `rawdata.raw_image` is non-null after unpack (mosaic buffer present).
    pub has_raw_image: bool,
    /// LibRaw / dcraw orientation (`sizes.flip`: 0 none, 3=180, 5=90CCW, 6=90CW).
    pub flip: i32,
}

impl SensorMeta {
    /// Standard Bayer CFA (not X-Trans / monochrome / unknown).
    pub fn is_bayer(&self) -> bool {
        self.filters != 0 && self.filters != !0 && self.colors == 3
    }
}

pub struct RawImage {
    raw_data: *mut sys::libraw_data_t,
}

unsafe impl Sync for RawImage {}

unsafe impl Send for RawImage {}

impl RawImage {
    pub fn open(buf: &[u8]) -> Result<Self> {
        let raw_data = unsafe { sys::libraw_init(0) };
        Error::check(unsafe {
            sys::libraw_open_buffer(raw_data, buf.as_ptr() as *const _, buf.len())
        })?;
        Ok(Self { raw_data })
    }

    pub fn unpack(&mut self) -> Result<()> {
        unsafe {
            let raw_param = &mut (*self.raw_data).rawparams;
            raw_param.use_rawspeed = 1;
            raw_param.max_raw_memory_mb = 1024;
            Error::check(sys::libraw_unpack(self.raw_data))
        }
    }

    pub fn extract_thumbs(&mut self) -> Result<Vec<ThumbnailImage>> {
        let mut thumbs = Thumbnails::new();
        for i in 0..self.as_ref().thumbs_list.thumbcount {
            Error::check(unsafe { sys::libraw_unpack_thumb_ex(self.raw_data, i) })?;
            let thumb = &self.as_ref().thumbnail;
            let thumb_image = ThumbnailImage {
                format: thumb.tformat.into(),
                width: thumb.twidth as _,
                height: thumb.theight as _,
                colors: thumb.tcolors as _,
                data: unsafe {
                    std::slice::from_raw_parts(thumb.thumb as *const u8, thumb.tlength as _)
                        .to_vec()
                },
            };
            thumbs.append(thumb_image);
        }
        Ok(thumbs.into_inner())
    }

    /// Enables or disables the ultra-fast pixel binning output.
    /// Must be called before `unpack()`.
    pub fn set_half_size(&mut self, enable: bool) {
        // Because we are touching a C-struct, Rust requires an unsafe block.
        // We are telling the compiler "I know what I'm doing with this pointer."
        unsafe {
            // Find what the underlying C pointer is named in their struct.
            // It might be `self.inner`, `self.libraw_data`, or `self.ptr`.
            // We access the `params` struct and flip the `half_size` integer.
            (*self.raw_data).params.half_size = if enable { 1 } else { 0 };
        }
    }

    pub fn width(&self) -> u32 {
        self.as_ref().sizes.width as _
    }

    pub fn height(&self) -> u32 {
        self.as_ref().sizes.height as _
    }

    pub fn pixels(&self) -> u32 {
        self.width() * self.height()
    }

    pub fn colors(&self) -> i32 {
        self.as_ref().idata.colors
    }

    pub fn iso_speed(&self) -> u32 {
        self.as_ref().other.iso_speed as _
    }

    pub fn shutter(&self) -> f32 {
        self.as_ref().other.shutter as _
    }

    pub fn aperture(&self) -> f32 {
        self.as_ref().other.aperture as _
    }

    pub fn focal_len(&self) -> f32 {
        self.as_ref().other.focal_len as _
    }

    pub fn datetime(&self) -> Option<DateTime<Local>> {
        let ts = self.as_ref().other.timestamp;
        Local.timestamp_opt(i64::from(ts), 0).single()
    }

    pub fn gps(&self) -> GpsInfo {
        self.as_ref().other.parsed_gps.into()
    }

    pub fn artist(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().other.artist[0] as *const _).to_string_lossy()
        }
    }

    pub fn desc(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().other.desc[0] as *const _).to_string_lossy()
        }
    }

    pub fn make(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.make[0] as *const _).to_string_lossy()
        }
    }

    pub fn model(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.model[0] as *const _).to_string_lossy()
        }
    }

    pub fn normalized_make(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.normalized_make[0] as *const _)
                .to_string_lossy()
        }
    }

    pub fn normalized_model(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.normalized_model[0] as *const _)
                .to_string_lossy()
        }
    }

    pub fn software(&self) -> Cow<'_, str> {
        unsafe {
            std::ffi::CStr::from_ptr(&self.as_ref().idata.software[0] as *const _).to_string_lossy()
        }
    }

    pub fn raw_count(&self) -> u32 {
        self.as_ref().idata.raw_count as _
    }

    pub fn dng_version(&self) -> u32 {
        self.as_ref().idata.dng_version as _
    }

    pub fn lens_info(&self) -> LensInfo {
        (&self.as_ref().lens).into()
    }

    pub fn full_info(&self) -> FullRawInfo {
        FullRawInfo {
            width: self.width(),
            height: self.height(),
            colors: self.colors(),
            iso_speed: self.iso_speed(),
            shutter: self.shutter(),
            aperture: self.aperture(),
            focal_len: self.focal_len(),
            datetime: self.datetime(),
            gps: self.gps(),
            artist: self.artist().to_string(),
            desc: self.desc().trim().into(),
            make: self.make().to_string(),
            model: self.model().to_string(),
            normalized_make: self.normalized_make().to_string(),
            normalized_model: self.normalized_model().to_string(),
            software: self.software().to_string(),
            raw_count: self.raw_count(),
            dng_version: self.dng_version(),
            lens_info: self.lens_info(),
        }
    }

    /// If possible, use the white balance from the camera.
    /// If not available, Auto-WB is used.
    pub fn set_use_camera_wb(&mut self, enable: bool) {
        unsafe {
            (*self.raw_data).params.use_camera_wb = if enable { 1 } else { 0 };
        }
    }

    /// On by default, call if you want to force use of DNG embedded matrix.
    pub fn set_use_camera_matrix(&mut self, enable: bool) {
        unsafe {
            // LibRaw: 0 = off, 3 = force use when available (historical rsraw default)
            (*self.raw_data).params.use_camera_matrix = if enable { 3 } else { 0 };
        }
    }

    /// Output colorspace for `dcraw_process` / `process` (`params.output_color`, LibRaw `-o`).
    ///
    /// Common values: 0 raw, **1 sRGB**, 2 Adobe, 3 Wide, 4 ProPhoto, 5 XYZ, 6 ACES.
    pub fn set_output_color(&mut self, space: OutputColor) {
        unsafe {
            (*self.raw_data).params.output_color = space as i32;
        }
    }

    /// Gamma curve power (`gamm[0]`) and toe slope (`gamm[1]`).
    /// For **linear** RGB samples use `(1.0, 1.0)`.
    pub fn set_gamma(&mut self, power: f32, toe_slope: f32) {
        unsafe {
            // Prefer C API when available
            sys::libraw_set_gamma(self.raw_data, 0, power);
            sys::libraw_set_gamma(self.raw_data, 1, toe_slope);
        }
    }

    /// Disable LibRaw auto-brightness stretch (`params.no_auto_bright`).
    pub fn set_no_auto_bright(&mut self, enable: bool) {
        unsafe {
            sys::libraw_set_no_auto_bright(self.raw_data, if enable { 1 } else { 0 });
        }
    }

    /// Highlight recovery mode (`params.highlight` / LibRaw `-H`).
    /// 0 = clip, 1 = unclip, 2 = blend, 3–9 = rebuild. Helps avoid magenta whites.
    pub fn set_highlight(&mut self, mode: i32) {
        unsafe {
            sys::libraw_set_highlight(self.raw_data, mode);
        }
    }

    /// Sensor / color metadata after `open` (richer after `unpack`).
    pub fn sensor_meta(&self) -> SensorMeta {
        let d = self.as_ref();
        let c = &d.color;
        SensorMeta {
            black: c.black,
            data_maximum: c.data_maximum,
            maximum: c.maximum,
            cam_mul: c.cam_mul,
            pre_mul: c.pre_mul,
            // rgb_cam: camera → sRGB-ish matrix used by LibRaw (3×4, first 3 cols)
            rgb_cam: [
                [c.rgb_cam[0][0], c.rgb_cam[0][1], c.rgb_cam[0][2]],
                [c.rgb_cam[1][0], c.rgb_cam[1][1], c.rgb_cam[1][2]],
                [c.rgb_cam[2][0], c.rgb_cam[2][1], c.rgb_cam[2][2]],
            ],
            filters: d.idata.filters,
            colors: d.idata.colors,
            raw_width: d.sizes.raw_width as u32,
            raw_height: d.sizes.raw_height as u32,
            width: d.sizes.width as u32,
            height: d.sizes.height as u32,
            left_margin: d.sizes.left_margin as u32,
            top_margin: d.sizes.top_margin as u32,
            raw_pitch: d.sizes.raw_pitch as u32,
            has_raw_image: !d.rawdata.raw_image.is_null(),
            flip: d.sizes.flip,
        }
    }

    /// Copy the **active-area** mosaic (`width`×`height`) as tightly packed `u16` samples.
    ///
    /// Requires [`unpack`](Self::unpack) first. Pitch may exceed `raw_width`; margins are
    /// applied so the returned buffer is contiguous row-major for the visible CFA.
    pub fn copy_mosaic_u16(&self) -> Result<Vec<u16>> {
        let meta = self.sensor_meta();
        if !meta.has_raw_image {
            return Err(Error::Data);
        }
        let w = meta.width as usize;
        let h = meta.height as usize;
        if w == 0 || h == 0 {
            return Err(Error::Data);
        }

        let d = self.as_ref();
        let ptr = d.rawdata.raw_image;
        if ptr.is_null() {
            return Err(Error::Data);
        }

        let pitch_samples = if meta.raw_pitch >= 2 {
            (meta.raw_pitch / 2) as usize
        } else {
            meta.raw_width as usize
        };
        let left = meta.left_margin as usize;
        let top = meta.top_margin as usize;
        let raw_w = meta.raw_width as usize;
        let raw_h = meta.raw_height as usize;

        if left + w > raw_w || top + h > raw_h {
            return Err(Error::BadCrop);
        }

        let mut out = Vec::with_capacity(w * h);
        unsafe {
            for row in 0..h {
                let src_row = ptr.add((top + row) * pitch_samples + left);
                let slice = std::slice::from_raw_parts(src_row, w);
                out.extend_from_slice(slice);
            }
        }
        Ok(out)
    }

    pub fn process<const D: BitDepth>(&mut self) -> Result<ProcessedImage<D>> {
        debug_assert!(D == BIT_DEPTH_8 || D == BIT_DEPTH_16);
        unsafe { (*self.raw_data).params.output_bps = D as i32 };
        Error::check(unsafe { sys::libraw_dcraw_process(self.raw_data) })?;

        let mut result = 0i32;
        let processed = unsafe { sys::libraw_dcraw_make_mem_image(self.raw_data, &mut result) };
        Error::check(result)?;
        Ok(unsafe { ProcessedImage::from_raw(processed) })
    }
}

impl Drop for RawImage {
    fn drop(&mut self) {
        unsafe { sys::libraw_close(self.raw_data) }
    }
}

impl AsRef<sys::libraw_data_t> for RawImage {
    fn as_ref(&self) -> &sys::libraw_data_t {
        unsafe { &*self.raw_data }
    }
}

impl AsMut<sys::libraw_data_t> for RawImage {
    fn as_mut(&mut self) -> &mut sys::libraw_data_t {
        unsafe { &mut *self.raw_data }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FullRawInfo {
    pub width: u32,
    pub height: u32,
    pub colors: i32,
    pub iso_speed: u32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub datetime: Option<DateTime<Local>>,
    pub gps: GpsInfo,
    pub artist: String,
    pub desc: String,
    pub make: String,
    pub model: String,
    pub normalized_make: String,
    pub normalized_model: String,
    pub software: String,
    pub raw_count: u32,
    pub dng_version: u32,
    pub lens_info: LensInfo,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rsraw_sys::{
        LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_Z, LibRaw_camera_mounts_LIBRAW_MOUNT_Sony_E,
    };

    use super::*;
    use crate::{lens::FocusType, processed::ImageFormat, Mounts};

    fn get_test_assets_path() -> PathBuf {
        let root: PathBuf = std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("must get manifest dir")
            .into();
        root.join("tests/assets")
    }

    #[test]
    fn test_raw_metadata() {
        let assets = get_test_assets_path();

        let test_cases = [
            (
                "test-z8.NEF",
                FullRawInfo {
                    width: 8280,
                    height: 5520,
                    colors: 3,
                    iso_speed: 250,
                    shutter: 1.0 / 100.0,
                    aperture: 3.5,
                    focal_len: 105.,
                    datetime: Local.with_ymd_and_hms(2024, 11, 4, 20, 11, 38).single(),
                    // Z8 NEF has GPS EXIF tags present but coordinates are
                    // zeroed (no fix). LibRaw still sets gpsparsed = 1, but
                    // since all coords are zero the result equals default.
                    gps: GpsInfo::default(),
                    artist: "HEXILEE".into(),
                    desc: "".into(),
                    make: "Nikon".into(),
                    model: "Z 8".into(),
                    normalized_make: "Nikon".into(),
                    normalized_model: "Z 8".into(),
                    software: "Ver.02.00".into(),
                    raw_count: 1,
                    dng_version: 0,
                    lens_info: LensInfo {
                        min_focal: 105.0,
                        max_focal: 105.0,
                        max_aperture_at_min_focal: 2.8,
                        max_aperture_at_max_focal: 2.8,
                        lens_make: "NIKON".into(),
                        lens_name: "NIKKOR Z MC 105mm f/2.8 VR S".into(),
                        lens_serial: "20044280".into(),
                        internal_lens_serial: "".into(),
                        focal_length_in_35mm_format: 105,
                        mounts: Mounts::from(LibRaw_camera_mounts_LIBRAW_MOUNT_Nikon_Z).to_string(),
                        focus_type: FocusType::Prime,
                        feture_pre: "AF".into(),
                        feture_suf: "".into(),
                    },
                },
            ),
            (
                "test-a7rm4.ARW",
                FullRawInfo {
                    width: 9568,
                    height: 6376,
                    colors: 3,
                    iso_speed: 320,
                    shutter: 1.0 / 500.0,
                    aperture: 4.0,
                    focal_len: 40.,
                    datetime: Local.with_ymd_and_hms(2023, 11, 17, 13, 0, 13).single(),
                    gps: Default::default(),
                    artist: "hexilee".into(),
                    desc: "".into(),
                    make: "Sony".into(),
                    model: "ILCE-7RM4".into(),
                    normalized_make: "Sony".into(),
                    normalized_model: "ILCE-7RM4".into(),
                    software: "ILCE-7RM4 v1.20".into(),
                    raw_count: 1,
                    dng_version: 0,
                    lens_info: LensInfo {
                        min_focal: 40.0,
                        max_focal: 40.0,
                        max_aperture_at_min_focal: 1.4,
                        max_aperture_at_max_focal: 1.4,
                        lens_make: "".into(),
                        lens_name: "40mm F1.4 DG HSM | Art 018".into(),
                        lens_serial: "".into(),
                        internal_lens_serial: "".into(),
                        focal_length_in_35mm_format: 40,
                        mounts: Mounts::from(LibRaw_camera_mounts_LIBRAW_MOUNT_Sony_E).to_string(),
                        focus_type: FocusType::Prime,
                        feture_pre: "".into(),
                        feture_suf: "".into(),
                    },
                },
            ),
        ];
        for (file, expected) in test_cases {
            let path = assets.join(file);
            let data = std::fs::read(path).unwrap();
            let raw_image = RawImage::open(&data).expect("opened");
            assert!(!raw_image.raw_data.is_null());
            let full_info = raw_image.full_info();
            assert_eq!(full_info, expected);
        }
    }

    #[test]
    fn test_thumbnails() {
        let assets = get_test_assets_path();
        let test_cases = [("test-z8.NEF",), ("test-a7rm4.ARW",)];
        for (file,) in test_cases {
            let path = assets.join(file);
            println!("{path:?}");
            let data = std::fs::read(path).unwrap();
            let mut raw_image = RawImage::open(&data).expect("opened");
            let thumbs = raw_image.extract_thumbs().expect("extracted");
            println!("{:?}", thumbs);
        }
    }

    #[test]
    fn test_processed() {
        let assets = get_test_assets_path();
        let test_cases = [
            (
                "test-z8.NEF",
                8280,
                5520,
                ImageFormat::Bitmap,
                3,
                16,
                274233600,
            ),
            (
                "test-a7rm4.ARW",
                9568,
                6376,
                ImageFormat::Bitmap,
                3,
                16,
                366033408,
            ),
        ];
        for (file, width, height, format, colors, bits, data_size) in test_cases {
            let path = assets.join(file);
            println!("{path:?}");
            let data = std::fs::read(path).unwrap();
            let mut raw_image = RawImage::open(&data).expect("opened");
            raw_image.unpack().expect("unpacked");
            let image = raw_image.process::<BIT_DEPTH_16>().expect("decoded");
            assert_eq!(image.width(), width);
            assert_eq!(image.height(), height);
            assert_eq!(image.image_format(), format);
            assert_eq!(image.colors(), colors);
            assert_eq!(image.bits(), bits);
            assert_eq!(image.data_size(), data_size);
        }
    }
}
