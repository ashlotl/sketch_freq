use core::f32;
use std::sync::{
    atomic::{AtomicI32, AtomicU32, Ordering},
    Arc,
};

use egui::{Rect, Slider, TextureHandle, TextureOptions};

use crate::{
    aabb::AABB,
    lv2_impl::{chunk_indices_from_sample_range, Synth},
    shared_data::{ChannelSelection, Data, DataWriter, Region},
};

pub struct Frontend {
    pub data: Arc<DataWriter>,
    pub synth: Arc<Synth>,

    pub alias_protection: f32,

    pub plot_texture_handle: Option<TextureHandle>,
    pub plot_texture_x_resolution: usize,
    pub plot_texture_buffer_rgba: Vec<u8>,

    /// Index of buffer to fill the red channel of plot texture with.
    pub plot_r_channel_binding: usize,

    /// Index of buffer to fill the green channel of plot texture with.
    pub plot_g_channel_binding: usize,

    /// Index of buffer to fill the blue channel of plot texture with.
    pub plot_b_channel_binding: usize,

    pub brush_radius_x: f32,
    pub brush_radius_y: f32,
    pub brush_color: ChannelSelection,
    pub brush_opacity: f32,

    pub eraser_radius_x: f32,
    pub eraser_radius_y: f32,
    pub eraser_color: ChannelSelection,

    pub utilization: Arc<AtomicU32>,
    pub debug: Arc<AtomicI32>,
}

impl Frontend {
    pub fn update_plot(&mut self, rerender: &AABB) {
        self.data.read_with(|data| {
            data.construct_rgba_buffer_from_plots(
                self.plot_r_channel_binding,
                self.plot_g_channel_binding,
                self.plot_b_channel_binding,
                self.plot_texture_x_resolution,
                &mut self.plot_texture_buffer_rgba,
                rerender.clone(),
            );
            self.plot_texture_handle.as_mut().unwrap().set(
                egui::ColorImage::from_rgba_unmultiplied(
                    [
                        self.plot_texture_x_resolution,
                        data.frequency_count as usize,
                    ],
                    &self.plot_texture_buffer_rgba,
                ),
                TextureOptions::NEAREST,
            )
        })
    }

    pub fn smooth_region(&self, region: &mut Region, margin: usize, smooth_until: f32) {
        fn smooth_sample(
            region: &mut Region,
            smooth_until: f32,
            freq_i: usize,
            sample_i: usize,
            forward: bool,
        ) {
            let next_or_prev = if forward { -1 } else { 1 };
            let index = freq_i * region.bounding_box.width() + sample_i;
            let other = region.data[(index as isize + next_or_prev) as usize];

            let max = other + smooth_until;
            let min = other - smooth_until;

            let value = region.data[index];
            region.data[index] = value.min(max).max(min);
        }

        for freq_i in 0..region.bounding_box.height() {
            //forward pass
            for sample_i in margin..region.bounding_box.width() - margin - 1 {
                smooth_sample(region, smooth_until, freq_i, sample_i, true);
            }
            //backward pass
            for sample_i in (margin..region.bounding_box.width() - margin).rev() {
                smooth_sample(region, smooth_until, freq_i, sample_i, false);
            }
        }
    }

    pub fn apply_brush(&self, brush_pos: (f32, f32), data: &mut Data) -> Option<AABB> {
        let Some(region) = data.for_pixel_in_brush_range(
            brush_pos,
            (self.brush_radius_x, self.brush_radius_y),
            &self.brush_color,
            |(x, y), value| {
                let dx = x as f32 - brush_pos.0;
                let dy = y as f32 - brush_pos.1;

                // check if inside ellipse
                if dx * dx / (self.brush_radius_x * self.brush_radius_x)
                    + dy * dy / (self.brush_radius_y * self.brush_radius_y)
                    > 1f32
                {
                    return value;
                }

                (value + (self.brush_opacity / 100f32)).min(1.0f32)
            },
        ) else {
            return None;
        };
        let mut affected = data.set_region(&region, &self.brush_color);
        const MARGIN: isize = 480;
        affected.expand_left(MARGIN);
        affected.expand_right(MARGIN);
        let mut region = data.get_region(affected, &self.brush_color);
        self.smooth_region(&mut region, MARGIN as usize, self.alias_protection);
        Some(data.set_region(&region, &self.brush_color))
    }

    pub fn apply_eraser(&self, pos: (f32, f32), data: &mut Data) -> Option<AABB> {
        let Some(region) = data.for_pixel_in_brush_range(
            pos,
            (self.eraser_radius_x, self.eraser_radius_y),
            &self.eraser_color,
            |(_x, _y), _value| 0f32,
        ) else {
            return None;
        };
        let mut affected = data.set_region(&region, &self.brush_color);
        const MARGIN: isize = 480;
        affected.expand_left(MARGIN);
        affected.expand_right(MARGIN);
        let mut region = data.get_region(affected, &self.brush_color);
        self.smooth_region(&mut region, MARGIN as usize, self.alias_protection);
        Some(data.set_region(&region, &self.eraser_color))
    }
}

impl eframe::App for Frontend {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();
        ctx.input(|i| i.viewport().close_requested());
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);

        self.plot_texture_handle.get_or_insert_with(|| {
            self.data.read_with(|data| {
                data.construct_rgba_buffer_from_plots(
                    self.plot_r_channel_binding,
                    self.plot_g_channel_binding,
                    self.plot_b_channel_binding,
                    self.plot_texture_x_resolution,
                    &mut self.plot_texture_buffer_rgba,
                    AABB::new((0, 0), (data.plot_length, data.frequency_count)).unwrap(),
                );

                ctx.load_texture(
                    "plot texture",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [self.plot_texture_x_resolution, data.frequency_count],
                        &self.plot_texture_buffer_rgba,
                    ),
                    TextureOptions::NEAREST,
                )
            })
        });

        #[derive(PartialEq)]
        enum MouseButton {
            Left,
            Right,
            None,
        }
        let mouse_button = ctx.input(|i| {
            if i.pointer.button_down(egui::PointerButton::Secondary) {
                return MouseButton::Right;
            } else if i.pointer.button_down(egui::PointerButton::Primary) {
                return MouseButton::Left;
            } else {
                return MouseButton::None;
            }
        });

        let mouse_pos = ctx.input(|i| i.pointer.latest_pos());

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(format!(
                "processing time utilization: {}%",
                self.utilization.load(Ordering::Relaxed)
            ));
            ui.horizontal(|ui| {
                ui.label("brush opacity: ");
                ui.add(Slider::new(&mut self.brush_opacity, 0f32..=100f32));
            });

            ui.horizontal(|ui| {
                ui.label("smoothing/alias protection: ");
                ui.add(Slider::new(&mut self.alias_protection, 0f32..=1f32));
            });

            ui.horizontal(|ui| {
                ui.label("brush radius x: ");
                ui.add(Slider::new(&mut self.brush_radius_x, 0.1..=48000f32));
            });

            ui.horizontal(|ui| {
                ui.label("brush radius y: ");
                ui.add(Slider::new(&mut self.brush_radius_y, 0.1f32..=500f32));
            });

            ui.horizontal(|ui| {
                ui.label("eraser radius x: ");
                ui.add(Slider::new(&mut self.eraser_radius_x, 0.1..=48000f32));
            });

            ui.horizontal(|ui| {
                ui.label("eraser radius y: ");
                ui.add(Slider::new(&mut self.eraser_radius_y, 0.1f32..=500f32));
            });

            ui.label(format!("debug: {}", self.debug.load(Ordering::Relaxed)));
            if ui.button("do recompute").clicked() {
                println!("triggering recompute of first chunk, should it exist...");
                self.data.write_with(&self.synth, |_data| vec![0]);
                println!("triggered")
            }
            let img_response = ui.image((
                self.plot_texture_handle.as_ref().unwrap().id(),
                ui.available_size(),
            ));

            if mouse_button == MouseButton::Left {
                if let Some(interaction_pos) = mouse_pos {
                    let mut affected_area = None;
                    self.data.write_with(&self.synth, |data| {
                        let pos = map_from_rect(
                            (interaction_pos.x, interaction_pos.y),
                            img_response.rect,
                            (data.plot_length as f32, data.frequency_count as f32),
                        );
                        affected_area = self.apply_brush(pos, data);
                        if let Some(affected_area) = &affected_area {
                            chunk_indices_from_sample_range((
                                affected_area.left(),
                                affected_area.right(),
                            ))
                        } else {
                            vec![]
                        }
                    });

                    if let Some(affected_area) = &affected_area {
                        self.update_plot(affected_area);
                    }
                }
            }

            if mouse_button == MouseButton::Right {
                if let Some(interaction_pos) = mouse_pos {
                    let mut affected_area = None;
                    self.data.write_with(&self.synth, |data| {
                        let pos = map_from_rect(
                            (interaction_pos.x, interaction_pos.y),
                            img_response.rect,
                            (data.plot_length as f32, data.frequency_count as f32),
                        );
                        affected_area = self.apply_eraser(pos, data);
                        if let Some(affected_area) = &affected_area {
                            chunk_indices_from_sample_range((
                                affected_area.left(),
                                affected_area.right(),
                            ))
                        } else {
                            vec![]
                        }
                    });

                    if let Some(affected_area) = &affected_area {
                        self.update_plot(affected_area);
                    }
                }
            }
        });
    }
}

fn map_from_rect(
    coord: (f32, f32),
    origin_rect: Rect,
    target_dimensions: (f32, f32),
) -> (f32, f32) {
    (
        (coord.0 - origin_rect.left()) / origin_rect.width() * target_dimensions.0,
        (coord.1 - origin_rect.top()) / origin_rect.height() * target_dimensions.1,
    )
}
