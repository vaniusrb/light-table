//! egui develop panel, toolbar actions, histogram + crop overlay.

use crate::crop::{
    self, CropAspect, CropHit, CropState,
};
use crate::develop::DevelopParams;

#[derive(Default)]
pub struct UiActions {
    pub open: bool,
    pub export: bool,
    pub reset: bool,
    pub fit: bool,
    pub toggle_crop: bool,
    pub reset_crop: bool,
}

/// Draw top toolbar + right develop panel.
/// Returns actions and the **central content rect** (image viewport, excluding panels).
pub fn draw_ui(
    ctx: &egui::Context,
    develop: &mut DevelopParams,
    crop: &mut CropState,
    image_label: Option<&str>,
    image_size: Option<(u32, u32)>,
    histogram: Option<&[u32; 1024]>,
    status: Option<&str>,
) -> (UiActions, egui::Rect) {
    let mut actions = UiActions::default();

    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("light-table");
            ui.separator();
            if ui.button("Open…").clicked() {
                actions.open = true;
            }
            if ui
                .add_enabled(image_label.is_some(), egui::Button::new("Export…"))
                .clicked()
            {
                actions.export = true;
            }
            if ui.button("Fit").clicked() {
                actions.fit = true;
            }
            if ui.button("Reset").clicked() {
                actions.reset = true;
            }
            ui.separator();
            let crop_label = if crop.editing { "Done crop" } else { "Crop" };
            if ui
                .add_enabled(image_label.is_some(), egui::Button::new(crop_label))
                .on_hover_text("Non-destructive crop (R to toggle)")
                .clicked()
            {
                actions.toggle_crop = true;
            }
            if crop.editing && ui.button("Reset crop").clicked() {
                actions.reset_crop = true;
            }
            ui.separator();
            if let Some(label) = image_label {
                ui.label(label);
                if let Some((w, h)) = image_size {
                    ui.label(format!("({w}×{h})"));
                }
                if !crop.rect.is_full_frame() {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 180, 80),
                        format!(
                            "cropped {:.0}%",
                            crop.rect.width() * crop.rect.height() * 100.0
                        ),
                    );
                }
            } else {
                ui.weak("No image open");
            }
            if let Some(msg) = status {
                ui.separator();
                ui.label(msg);
            }
        });
    });

    egui::SidePanel::right("develop")
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.heading("Develop");
            ui.separator();

            if let Some(bins) = histogram {
                ui.label("Histogram");
                draw_histogram(ui, bins);
                ui.separator();
            }

            // ── Crop & rotate (Lightroom-style) ───────────────────────
            section_header(ui, "Crop & rotate", || {
                crop.reset();
            });
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        crop.editing,
                        if crop.editing {
                            "Editing…"
                        } else {
                            "Edit crop"
                        },
                    )
                    .clicked()
                {
                    actions.toggle_crop = true;
                }
                if ui.small_button("Reset crop").clicked() {
                    actions.reset_crop = true;
                }
            });

            ui.label("Rotate");
            ui.horizontal(|ui| {
                if ui.button("⟲ 90°").on_hover_text("Rotate left (CCW)").clicked() {
                    crop.rotate_90_ccw();
                }
                if ui.button("⟳ 90°").on_hover_text("Rotate right (CW)").clicked() {
                    crop.rotate_90_cw();
                }
                if ui
                    .selectable_label(crop.flip_h, "Flip H")
                    .on_hover_text("Flip horizontal")
                    .clicked()
                {
                    crop.toggle_flip_h();
                }
                if ui
                    .selectable_label(crop.flip_v, "Flip V")
                    .on_hover_text("Flip vertical")
                    .clicked()
                {
                    crop.toggle_flip_v();
                }
            });
            slider_default(
                ui,
                &mut crop.angle_deg,
                -45.0..=45.0,
                "Straighten",
                0.0,
            );
            ui.weak("Straighten rotates under the crop · grid densifies when ≠ 0");

            ui.label("Aspect");
            ui.horizontal_wrapped(|ui| {
                for preset in [
                    CropAspect::Free,
                    CropAspect::Original,
                    CropAspect::Square,
                    CropAspect::R4x3,
                    CropAspect::R3x2,
                    CropAspect::R16x9,
                ] {
                    if ui
                        .selectable_label(crop.aspect == preset, preset.label())
                        .clicked()
                    {
                        crop.aspect = preset;
                        if let Some((w, h)) = image_size {
                            let ia = w as f32 / h.max(1) as f32;
                            crop.apply_aspect_preset(ia);
                        }
                    }
                }
            });
            ui.weak(format!(
                "L {:.0}%  T {:.0}%  R {:.0}%  B {:.0}%  ·  {}°",
                crop.rect.left * 100.0,
                crop.rect.top * 100.0,
                crop.rect.right * 100.0,
                crop.rect.bottom * 100.0,
                crop.angle_deg,
            ));
            if crop.editing {
                ui.colored_label(
                    egui::Color32::LIGHT_BLUE,
                    "Drag edges · grid for straighten",
                );
            }

            ui.separator();
            let d = DevelopParams::default();

            section_header(ui, "Light", || {
                develop.exposure = d.exposure;
                develop.contrast = d.contrast;
                develop.highlights = d.highlights;
                develop.shadows = d.shadows;
                develop.whites = d.whites;
                develop.blacks = d.blacks;
            });
            slider_default(ui, &mut develop.exposure, -5.0..=5.0, "Exposure", d.exposure);
            slider_default(ui, &mut develop.contrast, -1.0..=1.0, "Contrast", d.contrast);
            slider_default(ui, &mut develop.highlights, -1.0..=1.0, "Highlights", d.highlights);
            slider_default(ui, &mut develop.shadows, -1.0..=1.0, "Shadows", d.shadows);
            slider_default(ui, &mut develop.whites, -1.0..=1.0, "Whites", d.whites);
            slider_default(ui, &mut develop.blacks, -1.0..=1.0, "Blacks", d.blacks);

            ui.separator();
            section_header(ui, "White balance", || {
                develop.temperature = d.temperature;
                develop.tint = d.tint;
            });
            slider_default(ui, &mut develop.temperature, -1.0..=1.0, "Temp", d.temperature);
            slider_default(ui, &mut develop.tint, -1.0..=1.0, "Tint", d.tint);

            ui.separator();
            section_header(ui, "Presence", || {
                develop.vibrance = d.vibrance;
                develop.saturation = d.saturation;
            });
            slider_default(ui, &mut develop.vibrance, -1.0..=1.0, "Vibrance", d.vibrance);
            slider_default(
                ui,
                &mut develop.saturation,
                -1.0..=1.0,
                "Saturation",
                d.saturation,
            );

            ui.separator();
            section_header(ui, "Detail", || {
                develop.sharpen_amount = d.sharpen_amount;
                develop.sharpen_radius = d.sharpen_radius;
                develop.sharpen_detail = d.sharpen_detail;
                develop.denoise_luma = d.denoise_luma;
                develop.denoise_chroma = d.denoise_chroma;
            });
            slider_default(
                ui,
                &mut develop.sharpen_amount,
                0.0..=2.0,
                "Sharpen",
                d.sharpen_amount,
            );
            slider_default(
                ui,
                &mut develop.sharpen_radius,
                0.5..=3.0,
                "Radius",
                d.sharpen_radius,
            );
            slider_default(
                ui,
                &mut develop.sharpen_detail,
                0.0..=1.0,
                "Masking",
                d.sharpen_detail,
            );
            slider_default(
                ui,
                &mut develop.denoise_luma,
                0.0..=1.0,
                "Luminance NR",
                d.denoise_luma,
            );
            slider_default(
                ui,
                &mut develop.denoise_chroma,
                0.0..=1.0,
                "Color NR",
                d.denoise_chroma,
            );
            ui.weak("Unsharp mask + bilateral NR (GPU)");

            ui.separator();
            ui.weak("↻ = default · Crop is non-destructive");
        });

    // Remaining area after top + right panels = image viewport (matches GPU content rect)
    let content_rect = ctx.available_rect();

    (actions, content_rect)
}

/// Crop overlay: rule-of-thirds grid, border, handles.
///
/// Uses the same **content rect** as the GPU (central area only — not over the side panel).
pub fn draw_crop_overlay(
    ctx: &egui::Context,
    crop: &mut CropState,
    image_size: (u32, u32),
    content_rect: egui::Rect,
    drag_hit: &mut CropHit,
) {
    if !crop.editing {
        return;
    }

    let (img_w, img_h) = image_size;
    let image_aspect = img_w as f32 / img_h.max(1) as f32;

    // Letterbox image inside the content area only (same as GPU)
    let image_rect = crop::fitted_image_rect(content_rect, image_aspect);
    let crop_screen = egui::Rect::from_min_max(
        crop::image_uv_to_screen(egui::pos2(crop.rect.left, crop.rect.top), image_rect),
        crop::image_uv_to_screen(egui::pos2(crop.rect.right, crop.rect.bottom), image_rect),
    );

    // Clip drawing to content area so grid never covers the right panel
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("crop_overlay_layer"));
    let painter = ctx.layer_painter(layer).with_clip_rect(content_rect);

    // Border
    painter.rect_stroke(
        crop_screen,
        0.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(240, 240, 240)),
    );

    // Grid: rule-of-thirds, or denser (Lightroom-style) when straightening
    let dense = crop.angle_deg.abs() > 0.05;
    let divisions = if dense { 6 } else { 3 };
    let grid = egui::Color32::from_rgba_unmultiplied(255, 255, 255, if dense { 100 } else { 140 });
    let stroke = egui::Stroke::new(1.0, grid);
    for i in 1..divisions {
        let t = i as f32 / divisions as f32;
        let x = crop_screen.left() + crop_screen.width() * t;
        let y = crop_screen.top() + crop_screen.height() * t;
        painter.line_segment(
            [
                egui::pos2(x, crop_screen.top()),
                egui::pos2(x, crop_screen.bottom()),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(crop_screen.left(), y),
                egui::pos2(crop_screen.right(), y),
            ],
            stroke,
        );
    }
    // Center cross when straightening (helps level horizons)
    if dense {
        let mid = crop_screen.center();
        let cross = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 220, 80, 160));
        painter.line_segment(
            [
                egui::pos2(crop_screen.left(), mid.y),
                egui::pos2(crop_screen.right(), mid.y),
            ],
            cross,
        );
        painter.line_segment(
            [
                egui::pos2(mid.x, crop_screen.top()),
                egui::pos2(mid.x, crop_screen.bottom()),
            ],
            cross,
        );
    }

    // Corner / edge handles
    let hs = 6.0;
    let handle_col = egui::Color32::from_rgb(255, 220, 80);
    for c in [
        crop_screen.left_top(),
        crop_screen.right_top(),
        crop_screen.left_bottom(),
        crop_screen.right_bottom(),
    ] {
        painter.rect_filled(
            egui::Rect::from_center_size(c, egui::vec2(hs * 2.0, hs * 2.0)),
            1.0,
            handle_col,
        );
    }
    for m in [
        egui::pos2(crop_screen.center().x, crop_screen.top()),
        egui::pos2(crop_screen.center().x, crop_screen.bottom()),
        egui::pos2(crop_screen.left(), crop_screen.center().y),
        egui::pos2(crop_screen.right(), crop_screen.center().y),
    ] {
        painter.rect_filled(
            egui::Rect::from_center_size(m, egui::vec2(hs * 2.0, hs)),
            1.0,
            handle_col,
        );
    }

    // Interaction only inside the content / image area
    egui::Area::new(egui::Id::new("crop_drag_area"))
        .fixed_pos(content_rect.min)
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_min_size(content_rect.size());
            let response =
                ui.allocate_response(content_rect.size(), egui::Sense::click_and_drag());

            let handle_px = 14.0;
            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    // Only start drag if over the image (or near handles)
                    if image_rect.expand(handle_px).contains(pos) {
                        *drag_hit = crop::hit_test(crop_screen, pos, handle_px);
                    }
                }
            }
            if response.dragged() && *drag_hit != CropHit::None {
                let delta = response.drag_delta();
                let d_uv = egui::vec2(
                    delta.x / image_rect.width().max(1.0),
                    delta.y / image_rect.height().max(1.0),
                );
                crop::apply_drag(
                    &mut crop.rect,
                    *drag_hit,
                    d_uv,
                    crop.aspect,
                    image_aspect,
                );
            }
            if response.drag_stopped() {
                *drag_hit = CropHit::None;
            }

            if let Some(pos) = response.hover_pos() {
                if !image_rect.expand(handle_px).contains(pos) {
                    return;
                }
                let hit = if *drag_hit != CropHit::None {
                    *drag_hit
                } else {
                    crop::hit_test(crop_screen, pos, handle_px)
                };
                let icon = match hit {
                    CropHit::Left | CropHit::Right => egui::CursorIcon::ResizeHorizontal,
                    CropHit::Top | CropHit::Bottom => egui::CursorIcon::ResizeVertical,
                    CropHit::TopLeft | CropHit::BottomRight => egui::CursorIcon::ResizeNwSe,
                    CropHit::TopRight | CropHit::BottomLeft => egui::CursorIcon::ResizeNeSw,
                    CropHit::Move => egui::CursorIcon::Grab,
                    CropHit::None => egui::CursorIcon::Default,
                };
                ui.ctx().set_cursor_icon(icon);
            }
        });
}

/// Section title with a small “reset group to defaults” button.
fn section_header(ui: &mut egui::Ui, title: &str, mut reset_group: impl FnMut()) {
    ui.horizontal(|ui| {
        ui.label(title);
        if ui
            .small_button("↻")
            .on_hover_text(format!("Reset all {title} controls to defaults"))
            .clicked()
        {
            reset_group();
        }
    });
}

/// Slider + per-control “set to default” button (enabled when value ≠ default).
fn slider_default(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    default: f32,
) {
    ui.horizontal(|ui| {
        let changed = (*value - default).abs() > 1e-4;
        let resp = ui.add_enabled(
            changed,
            egui::Button::new("↻").small().min_size(egui::vec2(18.0, 18.0)),
        );
        if resp
            .on_hover_text(format!("Set {label} to default ({default:.2})"))
            .clicked()
        {
            *value = default;
        }
        ui.add(
            egui::Slider::new(value, range)
                .text(label)
                .fixed_decimals(2),
        );
    });
}

fn draw_histogram(ui: &mut egui::Ui, bins: &[u32; 1024]) {
    let height = 80.0;
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(20));
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(40)));

    let channels = [
        (0usize, egui::Color32::from_rgba_unmultiplied(230, 70, 70, 110)),
        (256, egui::Color32::from_rgba_unmultiplied(70, 200, 70, 110)),
        (512, egui::Color32::from_rgba_unmultiplied(70, 120, 240, 110)),
        (768, egui::Color32::from_rgba_unmultiplied(230, 230, 230, 140)),
    ];

    let n_bins = 256usize;
    let bar_w = rect.width() / n_bins as f32;

    for (offset, color) in channels {
        let channel = &bins[offset..offset + n_bins];
        // Scale by peak, but ignore a lone spike in bin 0 (pure black / letterbox)
        // so the rest of the curve is not flattened to the bottom.
        let mut max = 1u32;
        for (i, &c) in channel.iter().enumerate() {
            if i == 0 && c > 0 {
                // still allow pure-black mass, but prefer interior peak for scaling
                continue;
            }
            max = max.max(c);
        }
        // If almost everything is black, fall back to true max
        let true_max = channel.iter().copied().max().unwrap_or(1).max(1);
        if max <= 1 {
            max = true_max;
        }
        let mut second = 1u32;
        for &c in channel {
            if c < true_max {
                second = second.max(c);
            }
        }
        let scale = if true_max > second.saturating_mul(12).max(1) {
            (second as f32 * 1.35).max(1.0)
        } else {
            max.max(second).max(1) as f32
        };

        for (i, &count) in channel.iter().enumerate() {
            let v = (count as f32 / scale).clamp(0.0, 1.0);
            if v <= 0.0 {
                continue;
            }
            let x0 = rect.left() + i as f32 * bar_w;
            let h = v * rect.height();
            let bar = egui::Rect::from_min_max(
                egui::pos2(x0, rect.bottom() - h),
                egui::pos2(x0 + bar_w.max(1.0), rect.bottom()),
            );
            painter.rect_filled(bar, 0.0, color);
        }

        let stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 200),
        );
        let mut prev: Option<egui::Pos2> = None;
        for (i, &count) in channel.iter().enumerate() {
            let v = (count as f32 / scale).clamp(0.0, 1.0);
            let x = rect.left() + (i as f32 + 0.5) * bar_w;
            let y = rect.bottom() - v * rect.height();
            let p = egui::pos2(x, y);
            if let Some(q) = prev {
                painter.line_segment([q, p], stroke);
            }
            prev = Some(p);
        }
    }
}
