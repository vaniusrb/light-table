use crate::image_io::DecodedImage;

/// GPU-resident source image (immutable after upload for the session).
pub struct GpuImage {
    #[allow(dead_code)] // retained for future export / mip generation
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub label: String,
}

/// 1×1 mid-grey linear placeholder so bind groups are always valid.
pub fn create_dummy_source(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuImage {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dummy source"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    // ~0.18 mid grey in f16
    let pixel: [u16; 4] = [
        half::f16::from_f32(0.18).to_bits(),
        half::f16::from_f32(0.18).to_bits(),
        half::f16::from_f32(0.18).to_bits(),
        half::f16::from_f32(1.0).to_bits(),
    ];
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&pixel),
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuImage {
        texture,
        view,
        width: 1,
        height: 1,
        label: String::new(),
    }
}

/// Upload decoded linear float RGBA to an `Rgba16Float` GPU texture.
pub fn upload_source_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &DecodedImage,
) -> GpuImage {
    let width = image.width;
    let height = image.height;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("source image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    // Convert f32 → f16 for upload
    let mut f16_bytes = Vec::with_capacity(image.rgba_f32.len() * 2);
    for &v in &image.rgba_f32 {
        let bits = half::f16::from_f32(v).to_bits();
        f16_bytes.extend_from_slice(&bits.to_le_bytes());
    }

    let bytes_per_row = width * 8; // 4 × f16
    // wgpu requires bytes_per_row aligned to 256 for write_texture
    let align = 256u32;
    let padded_bpr = bytes_per_row.div_ceil(align) * align;

    let mut padded = vec![0u8; (padded_bpr * height) as usize];
    for y in 0..height {
        let src_off = (y * bytes_per_row) as usize;
        let dst_off = (y * padded_bpr) as usize;
        padded[dst_off..dst_off + bytes_per_row as usize]
            .copy_from_slice(&f16_bytes[src_off..src_off + bytes_per_row as usize]);
    }

    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(padded_bpr),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    log::info!(
        "Uploaded {} ({}×{}) to GPU (Rgba16Float, {:.1} MB)",
        image.label,
        width,
        height,
        (width as f64 * height as f64 * 8.0) / (1024.0 * 1024.0)
    );

    GpuImage {
        texture,
        view,
        width,
        height,
        label: image.label.clone(),
    }
}
