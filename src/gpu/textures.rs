use crate::demosaic::{DemosaicMode, MosaicBuffer};
use crate::gpu::pipelines::DemosaicPipelines;
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

/// Upload mono mosaic as `R32Float`, run GPU Bayer demosaic → `Rgba16Float` source.
///
/// `mode`: [`DemosaicMode::Half`] (2×2) or [`DemosaicMode::FullBilinear`] (1:1).
pub fn demosaic_mosaic_to_source(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    demosaic: &DemosaicPipelines,
    mosaic: &MosaicBuffer,
    mode: DemosaicMode,
) -> GpuImage {
    let mw = mosaic.width;
    let mh = mosaic.height;
    let (ow, oh) = mosaic.out_dims(mode);
    let params = mosaic.gpu_params(mode);

    // --- Mosaic texture (raw sample values as f32) ---
    let mosaic_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mosaic R32Float"),
        size: wgpu::Extent3d {
            width: mw,
            height: mh,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let mut f32_samples = Vec::with_capacity(mosaic.samples.len());
    for &s in &mosaic.samples {
        f32_samples.push(s as f32);
    }

    let bytes_per_row = mw * 4;
    let align = 256u32;
    let padded_bpr = bytes_per_row.div_ceil(align) * align;
    let mut padded = vec![0u8; (padded_bpr * mh) as usize];
    for y in 0..mh {
        let src_off = (y * mw) as usize;
        let dst_off = (y * padded_bpr) as usize;
        let row_bytes = bytemuck::cast_slice(&f32_samples[src_off..src_off + mw as usize]);
        padded[dst_off..dst_off + row_bytes.len()].copy_from_slice(row_bytes);
    }

    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &mosaic_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(padded_bpr),
            rows_per_image: Some(mh),
        },
        wgpu::Extent3d {
            width: mw,
            height: mh,
            depth_or_array_layers: 1,
        },
    );
    let mosaic_view = mosaic_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("mosaic nearest"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("demosaic params"),
        size: std::mem::size_of_val(&params) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));

    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("demosaic BG"),
        layout: &demosaic.bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&mosaic_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&nearest),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params_buf.as_entire_binding(),
            },
        ],
    });

    // --- Output linear sRGB ---
    let out_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("source demosaic Rgba16Float"),
        size: wgpu::Extent3d {
            width: ow,
            height: oh,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("demosaic encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("demosaic pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &out_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&demosaic.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }
    queue.submit(Some(encoder.finish()));

    log::info!(
        "GPU demosaic {} ({}): {}×{} mosaic → {}×{} linear sRGB ({:.1} MB)",
        mode.label(),
        mode as u32,
        mw,
        mh,
        ow,
        oh,
        (ow as f64 * oh as f64 * 8.0) / (1024.0 * 1024.0)
    );

    GpuImage {
        texture: out_tex,
        view: out_view,
        width: ow,
        height: oh,
        label: mosaic.label.clone(),
    }
}
