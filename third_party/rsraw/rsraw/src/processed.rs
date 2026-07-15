use std::{
    fmt::{self, Debug, Formatter},
    ops::Deref,
    slice,
};

use rsraw_sys as sys;

use crate::raw::{BitDepth, BIT_DEPTH_16, BIT_DEPTH_8};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Bitmap,
}

pub struct ProcessedImage<const D: BitDepth> {
    inner: *mut sys::libraw_processed_image_t,
}

unsafe impl Sync for ProcessedImage<BIT_DEPTH_8> {}
unsafe impl Send for ProcessedImage<BIT_DEPTH_8> {}
unsafe impl Sync for ProcessedImage<BIT_DEPTH_16> {}
unsafe impl Send for ProcessedImage<BIT_DEPTH_16> {}

impl<const D: BitDepth> ProcessedImage<D> {
    pub(crate) unsafe fn from_raw(ptr: *mut sys::libraw_processed_image_t) -> Self {
        debug_assert!(!ptr.is_null());
        Self { inner: ptr }
    }

    pub fn width(&self) -> u32 {
        unsafe { (*self.inner).width }.into()
    }

    pub fn height(&self) -> u32 {
        unsafe { (*self.inner).height }.into()
    }

    pub fn image_format(&self) -> ImageFormat {
        match unsafe { (*self.inner).type_ } {
            sys::LibRaw_image_formats_LIBRAW_IMAGE_JPEG => ImageFormat::Jpeg,
            sys::LibRaw_image_formats_LIBRAW_IMAGE_BITMAP => ImageFormat::Bitmap,
            _ => unreachable!(),
        }
    }

    pub fn colors(&self) -> u16 {
        unsafe { (*self.inner).colors }
    }

    pub fn bits(&self) -> u16 {
        unsafe { (*self.inner).bits }
    }

    pub fn data_size(&self) -> usize {
        unsafe { (*self.inner).data_size as usize }
    }
}

impl Deref for ProcessedImage<BIT_DEPTH_8> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe {
            slice::from_raw_parts(
                (*self.inner).data.as_ptr(),
                (*self.inner).data_size as usize,
            )
        }
    }
}

impl Deref for ProcessedImage<BIT_DEPTH_16> {
    type Target = [u16];

    fn deref(&self) -> &Self::Target {
        unsafe {
            debug_assert_eq!((*self.inner).data.as_ptr() as usize % 2, 0);

            slice::from_raw_parts(
                (*self.inner).data.as_ptr() as *const u16,
                (*self.inner).data_size as usize / 2,
            )
        }
    }
}

impl<const D: BitDepth> Drop for ProcessedImage<D> {
    fn drop(&mut self) {
        unsafe { sys::libraw_dcraw_clear_mem(self.inner) }
    }
}

impl<const D: BitDepth> Debug for ProcessedImage<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessedImage")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("image_format", &self.image_format())
            .field("colors", &self.colors())
            .field("bits", &self.bits())
            .field("data_size", &self.data_size())
            .finish()
    }
}
