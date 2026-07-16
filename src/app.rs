//! Application state: GPU resources, develop params, frame loop.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wgpu::util::DeviceExt;
use winit::{
    event::*,
    event_loop::EventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::WindowBuilder,
};

use crate::crop::{CropHit, CropState};
use crate::develop::{build_gpu_params, ContentViewport, DevelopParams, ViewState};
use crate::gpu::{
    create_dummy_source, create_shader_module, upload_source_texture, GpuContext, GpuImage,
    HistPipelines, PresentPipelines,
};
use crate::image_io::{self, DecodeQuality, DecodedImage};
use crate::ui::{self, UiActions};

/// Messages from the background decode thread (progressive RAW).
enum LoadMsg {
    Stage(DecodedImage),
    Done,
    Failed(String),
}

pub fn run() {
    init_logging();

    let event_loop = EventLoop::new().expect("event loop");
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("light-table")
            .with_inner_size(winit::dpi::LogicalSize::new(1400, 900))
            .build(&event_loop)
            .expect("window"),
    );

    let mut state = pollster::block_on(App::new(window.clone()));

    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(winit::event_loop::ControlFlow::Poll);

            match event {
                Event::WindowEvent { event, window_id } if window_id == state.window.id() => {
                    let egui_response = state
                        .egui_winit_state
                        .on_window_event(&state.window, &event);

                    if !egui_response.consumed {
                        state.handle_window_event(&event);
                    }

                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(size) => state.resize(size),
                        WindowEvent::RedrawRequested => match state.render() {
                            Ok(()) => {}
                            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                state.resize(state.gpu.size);
                            }
                            Err(wgpu::SurfaceError::OutOfMemory) => {
                                log::error!("Out of GPU memory");
                                elwt.exit();
                            }
                            Err(e) => log::warn!("surface error: {e:?}"),
                        },
                        _ => {}
                    }
                }
                Event::AboutToWait => {
                    state.window.request_redraw();
                }
                _ => {}
            }
        })
        .expect("event loop run");
}

fn init_logging() {
    env_logger::Builder::from_default_env()
        .filter_module("sctk_adwaita", log::LevelFilter::Off)
        .filter_module("wgpu_hal::gles", log::LevelFilter::Off)
        .filter_level(log::LevelFilter::Info)
        .init();
}

struct App {
    window: Arc<winit::window::Window>,
    gpu: GpuContext,

    present: PresentPipelines,
    hist: HistPipelines,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    hist_bins_buffer: wgpu::Buffer,
    hist_staging: wgpu::Buffer,

    source: GpuImage,
    present_bind_group: wgpu::BindGroup,
    hist_bind_group: wgpu::BindGroup,

    has_image: bool,
    develop: DevelopParams,
    prev_develop: DevelopParams,
    view: ViewState,
    crop: CropState,
    crop_drag: CropHit,
    /// Central UI area (normalized later for GPU); excludes toolbar + side panel.
    content_viewport: ContentViewport,
    hist_dirty: bool,
    histogram: [u32; 1024],

    dragging: bool,
    last_cursor: Option<(f64, f64)>,

    egui_ctx: egui::Context,
    egui_winit_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    status: Option<(String, Instant)>,
    /// Host copy for export cold path only (preview develop is GPU).
    export_pixels: Option<DecodedImage>,
    /// Original file path for full-res re-decode on export.
    open_path: Option<PathBuf>,
    /// Progressive load channel (thumbnail → half-size).
    load_rx: Option<Receiver<LoadMsg>>,
    loading: bool,
}

impl App {
    async fn new(window: Arc<winit::window::Window>) -> Self {
        let gpu = GpuContext::new(window.clone()).await;
        let shader = create_shader_module(&gpu.device);
        let present = PresentPipelines::new(&gpu.device, &shader, gpu.config.format);
        let hist = HistPipelines::new(&gpu.device, &shader);

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let develop = DevelopParams::default();
        let view = ViewState::default();
        let crop = CropState::default();
        let content_viewport = ContentViewport::default();
        let gpu_params = build_gpu_params(
            &develop,
            &view,
            &crop,
            content_viewport,
            1,
            1,
            false,
        );
        let params_buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("develop params"),
                contents: bytemuck::bytes_of(&gpu_params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let hist_bins_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist bins"),
            size: 1024 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let hist_staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist staging"),
            size: 1024 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let source = create_dummy_source(&gpu.device, &gpu.queue);
        let present_bind_group = make_present_bg(
            &gpu.device,
            &present.bgl,
            &source.view,
            &sampler,
            &params_buffer,
        );
        let hist_bind_group = make_hist_bg(
            &gpu.device,
            &hist.bgl,
            &source.view,
            &sampler,
            &params_buffer,
            &hist_bins_buffer,
        );

        let egui_ctx = egui::Context::default();
        let egui_winit_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.config.format, None, 1);

        Self {
            window,
            gpu,
            present,
            hist,
            sampler,
            params_buffer,
            hist_bins_buffer,
            hist_staging,
            source,
            present_bind_group,
            hist_bind_group,
            has_image: false,
            prev_develop: develop.clone(),
            develop,
            view,
            crop,
            crop_drag: CropHit::None,
            content_viewport,
            hist_dirty: true,
            histogram: [0; 1024],
            dragging: false,
            last_cursor: None,
            egui_ctx,
            egui_winit_state,
            egui_renderer,
            status: None,
            export_pixels: None,
            open_path: None,
            load_rx: None,
            loading: false,
        }
    }

    fn toggle_crop_mode(&mut self) {
        if !self.has_image {
            return;
        }
        self.crop.editing = !self.crop.editing;
        self.view.fit();
        self.crop_drag = CropHit::None;
        if self.crop.editing {
            self.set_status("Crop mode — drag edges/corners · R or Done crop to finish");
        } else {
            self.set_status("Crop applied (non-destructive)");
        }
    }

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        self.gpu.resize(new_size);
    }

    fn handle_window_event(&mut self, event: &WindowEvent) {
        if self.egui_ctx.is_pointer_over_area()
            && matches!(
                event,
                WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
            )
        {
            return;
        }

        // In crop-edit mode, pan/zoom disabled — crop overlay handles dragging
        if self.crop.editing {
            if let WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyR),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } = event
            {
                self.toggle_crop_mode();
            }
            if let WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } = event
            {
                self.toggle_crop_mode();
            }
            return;
        }

        match event {
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.dragging = *state == ElementState::Pressed;
                if !self.dragging {
                    self.last_cursor = None;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.dragging {
                    if let Some((lx, ly)) = self.last_cursor {
                        let dx = (position.x - lx) as f32;
                        let dy = (position.y - ly) as f32;
                        let inv_h = 1.0 / self.gpu.size.height.max(1) as f32;
                        let z = self.view.zoom.max(0.01);
                        self.view.pan_x -= dx * inv_h / z;
                        self.view.pan_y -= dy * inv_h / z;
                    }
                    self.last_cursor = Some((position.x, position.y));
                } else {
                    self.last_cursor = Some((position.x, position.y));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.01,
                };
                self.view.zoom = (self.view.zoom * (1.0 + scroll * 0.1)).clamp(0.1, 32.0);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyO),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => self.open_dialog(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyF),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => self.view.fit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyR),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => self.toggle_crop_mode(),
            _ => {}
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Images",
                &[
                    "jpg", "jpeg", "png", "tif", "tiff", "webp", "bmp", "cr2", "cr3", "nef", "arw",
                    "dng", "raf", "rw2", "orf", "pef", "raw",
                ],
            )
            .add_filter(
                "RAW (LibRaw / rsraw)",
                &["cr2", "cr3", "nef", "arw", "dng", "raf", "rw2"],
            )
            .pick_file()
        {
            self.open_path(path);
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        self.set_status(format!("Loading {}…", path.display()));
        self.open_path = Some(path.clone());
        self.loading = true;

        // Raster files: single-shot on a worker so the UI stays responsive
        if !image_io::is_raw_path(&path) {
            let (tx, rx) = mpsc::channel();
            self.load_rx = Some(rx);
            std::thread::spawn(move || {
                match image_io::load_image(&path) {
                    Ok(img) => {
                        let _ = tx.send(LoadMsg::Stage(img));
                        let _ = tx.send(LoadMsg::Done);
                    }
                    Err(e) => {
                        let _ = tx.send(LoadMsg::Failed(e.to_string()));
                    }
                }
            });
            return;
        }

        // RAW: progressive thumbnail → half-size demosaic (background thread)
        let (tx, rx) = mpsc::channel();
        self.load_rx = Some(rx);
        std::thread::spawn(move || {
            let send = |img: DecodedImage| {
                let _ = tx.send(LoadMsg::Stage(img));
            };
            match image_io::load_raw_progressive(&path, send) {
                Ok(()) => {
                    let _ = tx.send(LoadMsg::Done);
                }
                Err(e) => {
                    let _ = tx.send(LoadMsg::Failed(e.to_string()));
                }
            }
        });
    }

    /// Poll background decode; apply newest stage to GPU.
    fn poll_load(&mut self) {
        let Some(rx) = self.load_rx.as_ref() else {
            return;
        };

        // Drain all ready messages so we jump to the latest stage this frame
        let mut latest: Option<DecodedImage> = None;
        let mut done = false;
        let mut failed: Option<String> = None;

        loop {
            match rx.try_recv() {
                Ok(LoadMsg::Stage(img)) => latest = Some(img),
                Ok(LoadMsg::Done) => done = true,
                Ok(LoadMsg::Failed(e)) => failed = Some(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }

        if let Some(img) = latest {
            let first = !self.has_image;
            self.apply_decoded(img, first);
        }

        if let Some(e) = failed {
            self.set_status(format!("Open failed: {e}"));
            log::error!("open failed: {e}");
            self.loading = false;
            self.load_rx = None;
        } else if done {
            self.loading = false;
            self.load_rx = None;
            if self.has_image {
                let q = self
                    .export_pixels
                    .as_ref()
                    .map(|p| p.quality.label())
                    .unwrap_or("?");
                self.set_status(format!("Ready ({q}) — {}", self.source.label));
            }
        }
    }

    fn apply_decoded(&mut self, decoded: DecodedImage, reset_view: bool) {
        let quality = decoded.quality;
        let label = decoded.label.clone();
        let dims = (decoded.width, decoded.height);

        let gpu_img = upload_source_texture(&self.gpu.device, &self.gpu.queue, &decoded);
        self.source = gpu_img;
        self.has_image = true;
        if reset_view {
            self.view.fit();
            self.develop.reset();
            self.prev_develop = self.develop.clone();
            self.crop = CropState::default();
            self.crop_drag = CropHit::None;
        }
        self.hist_dirty = true;

        self.present_bind_group = make_present_bg(
            &self.gpu.device,
            &self.present.bgl,
            &self.source.view,
            &self.sampler,
            &self.params_buffer,
        );
        self.hist_bind_group = make_hist_bg(
            &self.gpu.device,
            &self.hist.bgl,
            &self.source.view,
            &self.sampler,
            &self.params_buffer,
            &self.hist_bins_buffer,
        );

        self.export_pixels = Some(decoded);
        self.set_status(format!(
            "{} {}×{} ({})",
            match quality {
                DecodeQuality::Thumbnail => "Preview",
                DecodeQuality::HalfSize => "Develop proxy",
                DecodeQuality::Full => "Opened",
            },
            dims.0,
            dims.1,
            label
        ));
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now() + Duration::from_secs(4)));
    }

    fn export_dialog(&mut self) {
        if self.export_pixels.is_none() && self.open_path.is_none() {
            self.set_status("Nothing to export");
            return;
        }

        if let Some(path) = rfd::FileDialog::new()
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter("PNG", &["png"])
            .set_file_name("export.jpg")
            .save_file()
        {
            // Prefer full-res re-decode from original RAW path when we only have a proxy
            let quality = self.export_pixels.as_ref().map(|d| d.quality);
            let src_path = self.open_path.clone();
            let decoded = match (quality, src_path) {
                (Some(DecodeQuality::Full), _) | (None, None) => self.export_pixels.clone(),
                (_, Some(src)) if image_io::is_raw_path(&src) => {
                    self.set_status("Exporting full-res RAW (this may take a while)…");
                    match image_io::load_raw_full(&src) {
                        Ok(full) => Some(full),
                        Err(e) => {
                            self.set_status(format!("Full-res decode failed, using proxy: {e}"));
                            self.export_pixels.clone()
                        }
                    }
                }
                _ => self.export_pixels.clone(),
            };

            let Some(decoded) = decoded else {
                self.set_status("Nothing to export");
                return;
            };

            let developed = apply_develop_cpu(&decoded, &self.develop);
            // Non-destructive crop + rotate at export
            let needs_geom = !self.crop.rect.is_full_frame()
                || self.crop.angle_deg.abs() > 1e-3
                || self.crop.orient_90 % 4 != 0
                || self.crop.flip_h
                || self.crop.flip_v;
            let (ew, eh, cropped) = if needs_geom {
                crate::crop::render_crop_rotate(
                    &developed,
                    decoded.width,
                    decoded.height,
                    &self.crop,
                )
            } else {
                (decoded.width, decoded.height, developed)
            };
            match image_io::save_srgb_image(&path, ew, eh, &cropped) {
                Ok(()) => self.set_status(format!(
                    "Exported {} ({}×{}, {})",
                    path.display(),
                    ew,
                    eh,
                    decoded.quality.label()
                )),
                Err(e) => self.set_status(format!("Export failed: {e}")),
            }
        }
    }

    fn upload_params(&self) {
        let params = build_gpu_params(
            &self.develop,
            &self.view,
            &self.crop,
            self.content_viewport,
            self.source.width,
            self.source.height,
            self.has_image,
        );
        self.gpu
            .queue
            .write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    fn develop_changed(&self) -> bool {
        let a = &self.develop;
        let b = &self.prev_develop;
        a.exposure != b.exposure
            || a.contrast != b.contrast
            || a.highlights != b.highlights
            || a.shadows != b.shadows
            || a.whites != b.whites
            || a.blacks != b.blacks
            || a.temperature != b.temperature
            || a.tint != b.tint
            || a.vibrance != b.vibrance
            || a.saturation != b.saturation
            || a.denoise_luma != b.denoise_luma
            || a.denoise_chroma != b.denoise_chroma
            || a.sharpen_amount != b.sharpen_amount
            || a.sharpen_radius != b.sharpen_radius
            || a.sharpen_detail != b.sharpen_detail
    }

    fn run_histogram(&mut self) {
        if !self.has_image {
            return;
        }

        self.gpu
            .queue
            .write_buffer(&self.hist_bins_buffer, 0, bytemuck::bytes_of(&[0u32; 1024]));

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hist encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("hist pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.hist.pipeline);
            pass.set_bind_group(0, &self.hist_bind_group, &[]);
            pass.dispatch_workgroups(32, 32, 1);
        }
        encoder.copy_buffer_to_buffer(&self.hist_bins_buffer, 0, &self.hist_staging, 0, 1024 * 4);
        self.gpu.queue.submit(Some(encoder.finish()));

        let slice = self.hist_staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.gpu.device.poll(wgpu::Maintain::Wait);
        if rx.recv().ok().and_then(|r| r.ok()).is_some() {
            let data = slice.get_mapped_range();
            let bins: &[u32] = bytemuck::cast_slice(&data);
            self.histogram.copy_from_slice(bins);
            drop(data);
            self.hist_staging.unmap();
        }
        self.hist_dirty = false;
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.poll_load();

        // Crop-edit must match GPU fit (no pan/zoom) or the bright rect drifts from the grid
        if self.crop.editing {
            self.view.fit();
        }

        if self.develop_changed() {
            self.hist_dirty = true;
            self.prev_develop = self.develop.clone();
        }

        self.upload_params();
        if self.hist_dirty {
            self.run_histogram();
        }

        let raw_input = self.egui_winit_state.take_egui_input(&self.window);

        let status_owned = self.status.as_ref().and_then(|(msg, until)| {
            if Instant::now() < *until {
                Some(msg.clone())
            } else {
                None
            }
        });

        let mut actions = UiActions::default();
        let full_output = {
            let develop = &mut self.develop;
            let crop = &mut self.crop;
            let crop_drag = &mut self.crop_drag;
            let image_label = if self.has_image {
                Some(self.source.label.as_str())
            } else {
                None
            };
            let image_size = if self.has_image {
                Some((self.source.width, self.source.height))
            } else {
                None
            };
            let hist = if self.has_image {
                Some(&self.histogram)
            } else {
                None
            };
            let status = status_owned.as_deref();
            let img_dims = (self.source.width, self.source.height);

            let mut content_rect = egui::Rect::NOTHING;
            let full_output = self.egui_ctx.run(raw_input, |ctx| {
                let (a, content) =
                    ui::draw_ui(ctx, develop, crop, image_label, image_size, hist, status);
                actions = a;
                content_rect = content;
                if crop.editing && image_size.is_some() {
                    ui::draw_crop_overlay(ctx, crop, img_dims, content, crop_drag);
                }
            });
            // Update GPU content viewport to match egui central area
            let screen = self.egui_ctx.screen_rect();
            if content_rect.width() > 1.0 && content_rect.height() > 1.0 {
                self.content_viewport = ContentViewport::from_rects(screen, content_rect);
            }
            full_output
        };

        if actions.open {
            self.open_dialog();
        }
        if actions.export {
            self.export_dialog();
        }
        if actions.reset {
            self.develop.reset();
            self.hist_dirty = true;
        }
        if actions.fit {
            self.view.fit();
        }
        if actions.toggle_crop {
            self.toggle_crop_mode();
        }
        if actions.reset_crop {
            self.crop.reset();
            self.set_status("Crop reset to full frame");
        }

        self.egui_winit_state
            .handle_platform_output(&self.window, full_output.platform_output);

        let clipped_primitives = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }

        // Re-upload params after UI may have changed develop
        self.upload_params();

        let output = self.gpu.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.gpu.config.width, self.gpu.config.height],
            pixels_per_point: self.window.scale_factor() as f32,
        };

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        let egui_cmd_bufs = self.egui_renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        // Present: develop + pan/zoom into swapchain
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.06,
                            g: 0.06,
                            b: 0.07,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.present.pipeline);
            pass.set_bind_group(0, &self.present_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // egui overlay
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.egui_renderer
                .render(&mut pass, &clipped_primitives, &screen_descriptor);
        }

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        self.gpu
            .queue
            .submit(egui_cmd_bufs.into_iter().chain(std::iter::once(encoder.finish())));
        output.present();

        Ok(())
    }
}

fn make_present_bg(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("present BG"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

fn make_hist_bg(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    params: &wgpu::Buffer,
    bins: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hist BG"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: bins.as_entire_binding(),
            },
        ],
    })
}

/// CPU mirror of the GPU develop chain for full-res export (cold path).
fn apply_develop_cpu(image: &DecodedImage, p: &DevelopParams) -> Vec<f32> {
    let w = image.width as usize;
    let h = image.height as usize;
    let src = &image.rgba_f32;

    // Optional denoise then unsharp (matches GPU order; slower on full-res)
    let mut filtered = if p.denoise_luma > 0.001 || p.denoise_chroma > 0.001 {
        denoise_cpu(src, w, h, p.denoise_luma, p.denoise_chroma)
    } else {
        src.clone()
    };
    if p.sharpen_amount > 0.001 {
        filtered = sharpen_cpu(
            &filtered,
            w,
            h,
            p.sharpen_amount,
            p.sharpen_radius,
            p.sharpen_detail,
        );
    }

    let mut out = Vec::with_capacity(filtered.len());
    for chunk in filtered.chunks_exact(4) {
        let mut r = chunk[0];
        let mut g = chunk[1];
        let mut b = chunk[2];
        let a = chunk[3];

        // WB
        let r_gain = 1.0 + p.temperature * 0.35 - p.tint * 0.08;
        let g_gain = 1.0 + p.tint * 0.20;
        let b_gain = 1.0 - p.temperature * 0.35 - p.tint * 0.08;
        r *= r_gain;
        g *= g_gain;
        b *= b_gain;

        // Exposure
        let m = 2f32.powf(p.exposure);
        r *= m;
        g *= m;
        b *= m;

        // Contrast around 0.18
        let pivot = 0.18;
        let c = 1.0 + p.contrast;
        r = (r - pivot) * c + pivot;
        g = (g - pivot) * c + pivot;
        b = (b - pivot) * c + pivot;

        // Tone regions
        let luma = r * 0.2126 + g * 0.7152 + b * 0.0722;
        let shadow_w = (1.0 - luma * 2.0).clamp(0.0, 1.0);
        let highlight_w = ((luma - 0.5) * 2.0).clamp(0.0, 1.0);
        r += p.shadows * shadow_w * 0.25 + p.highlights * highlight_w * 0.25;
        g += p.shadows * shadow_w * 0.25 + p.highlights * highlight_w * 0.25;
        b += p.shadows * shadow_w * 0.25 + p.highlights * highlight_w * 0.25;
        r = r * (1.0 + p.whites * 0.15) + p.blacks * 0.1;
        g = g * (1.0 + p.whites * 0.15) + p.blacks * 0.1;
        b = b * (1.0 + p.whites * 0.15) + p.blacks * 0.1;

        // Vibrance / sat
        let luma = r * 0.2126 + g * 0.7152 + b * 0.0722;
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let sat = if max_c > 1e-5 {
            (max_c - min_c) / max_c
        } else {
            0.0
        };
        let vib = p.vibrance * (1.0 - sat);
        r = luma + (r - luma) * (1.0 + vib);
        g = luma + (g - luma) * (1.0 + vib);
        b = luma + (b - luma) * (1.0 + vib);
        r = luma + (r - luma) * (1.0 + p.saturation);
        g = luma + (g - luma) * (1.0 + p.saturation);
        b = luma + (b - luma) * (1.0 + p.saturation);

        out.extend_from_slice(&[r.max(0.0), g.max(0.0), b.max(0.0), a]);
    }
    out
}

fn luma_rgb(r: f32, g: f32, b: f32) -> f32 {
    r * 0.2126 + g * 0.7152 + b * 0.0722
}

/// Simple 5×5 bilateral-style denoise for export (CPU).
fn denoise_cpu(src: &[f32], w: usize, h: usize, luma_str: f32, chroma_str: f32) -> Vec<f32> {
    let luma_str = luma_str.clamp(0.0, 1.0);
    let chroma_str = chroma_str.clamp(0.0, 1.0);
    let sigma_s = 0.6 + luma_str * 2.2;
    let sigma_r = 0.02 + luma_str * 0.12;
    let inv_2s2 = 1.0 / (2.0 * sigma_s * sigma_s);
    let inv_2r2 = 1.0 / (2.0 * sigma_r * sigma_r);

    let mut out = src.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let cr = src[i];
            let cg = src[i + 1];
            let cb = src[i + 2];
            let ca = src[i + 3];
            let cl = luma_rgb(cr, cg, cb);

            let mut sum_l = 0.0f32;
            let mut w_l = 0.0f32;
            let mut sum_r = 0.0f32;
            let mut sum_g = 0.0f32;
            let mut sum_b = 0.0f32;
            let mut w_c = 0.0f32;

            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                    let ny = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                    let j = (ny * w + nx) * 4;
                    let nr = src[j];
                    let ng = src[j + 1];
                    let nb = src[j + 2];
                    let nl = luma_rgb(nr, ng, nb);
                    let d2 = (dx * dx + dy * dy) as f32;
                    let ws = (-d2 * inv_2s2).exp();
                    let dl = nl - cl;
                    let wr = (-(dl * dl) * inv_2r2).exp();
                    let weight = ws * wr;
                    sum_l += nl * weight;
                    w_l += weight;
                    let wc = (-d2 * inv_2s2 * 0.45).exp();
                    sum_r += nr * wc;
                    sum_g += ng * wc;
                    sum_b += nb * wc;
                    w_c += wc;
                }
            }

            let filt_l = if w_l > 1e-6 { sum_l / w_l } else { cl };
            let fr = if w_c > 1e-6 { sum_r / w_c } else { cr };
            let fg = if w_c > 1e-6 { sum_g / w_c } else { cg };
            let fb = if w_c > 1e-6 { sum_b / w_c } else { cb };
            let fl = luma_rgb(fr, fg, fb);

            let out_l = cl * (1.0 - luma_str) + filt_l * luma_str;
            let c_chr_r = cr - cl;
            let c_chr_g = cg - cl;
            let c_chr_b = cb - cl;
            let f_chr_r = fr - fl;
            let f_chr_g = fg - fl;
            let f_chr_b = fb - fl;
            let or_ = out_l + c_chr_r * (1.0 - chroma_str) + f_chr_r * chroma_str;
            let og = out_l + c_chr_g * (1.0 - chroma_str) + f_chr_g * chroma_str;
            let ob = out_l + c_chr_b * (1.0 - chroma_str) + f_chr_b * chroma_str;
            out[i] = or_;
            out[i + 1] = og;
            out[i + 2] = ob;
            out[i + 3] = ca;
        }
    }
    out
}

/// Unsharp mask on luma for export (CPU, mirrors GPU).
fn sharpen_cpu(
    src: &[f32],
    w: usize,
    h: usize,
    amount: f32,
    radius: f32,
    detail: f32,
) -> Vec<f32> {
    let amount = amount.clamp(0.0, 2.0);
    let radius = radius.clamp(0.3, 3.0);
    let thr = detail.clamp(0.0, 1.0) * 0.04;
    let sigma = (radius * 0.5).max(0.35);
    let inv_2s2 = 1.0 / (2.0 * sigma * sigma);

    let mut out = src.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let cr = src[i];
            let cg = src[i + 1];
            let cb = src[i + 2];
            let ca = src[i + 3];
            let cl = luma_rgb(cr, cg, cb);

            let mut sum_r = 0.0f32;
            let mut sum_g = 0.0f32;
            let mut sum_b = 0.0f32;
            let mut wsum = 0.0f32;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let ox = dx as f32 * radius * 0.5;
                    let oy = dy as f32 * radius * 0.5;
                    let nx = (x as f32 + ox).round().clamp(0.0, (w - 1) as f32) as usize;
                    let ny = (y as f32 + oy).round().clamp(0.0, (h - 1) as f32) as usize;
                    let j = (ny * w + nx) * 4;
                    let d2 = ox * ox + oy * oy;
                    let weight = (-d2 * inv_2s2).exp();
                    sum_r += src[j] * weight;
                    sum_g += src[j + 1] * weight;
                    sum_b += src[j + 2] * weight;
                    wsum += weight;
                }
            }
            let bl = if wsum > 1e-6 {
                luma_rgb(sum_r / wsum, sum_g / wsum, sum_b / wsum)
            } else {
                cl
            };

            let mut detail_v = cl - bl;
            let ad = detail_v.abs();
            if ad < thr {
                detail_v *= ad / thr.max(1e-6);
            }
            let out_l = cl + detail_v * amount;
            out[i] = out_l + (cr - cl);
            out[i + 1] = out_l + (cg - cl);
            out[i + 2] = out_l + (cb - cl);
            out[i + 3] = ca;
        }
    }
    out
}
