use core::f32;
use std::sync::{
    atomic::{AtomicI32, AtomicU32, Ordering},
    Arc,
};

use egui::{Rect, TextureHandle, TextureOptions};

use crate::shared_data::{ChannelSelection, Data, DataWriter};

pub struct Frontend {
    pub data: Arc<DataWriter>,
    pub plot_texture_handle: Option<TextureHandle>,

    /// Index of buffer to fill the red channel of plot texture with.
    pub plot_r_channel_binding: usize,

    /// Index of buffer to fill the green channel of plot texture with.
    pub plot_g_channel_binding: usize,

    /// Index of buffer to fill the blue channel of plot texture with.
    pub plot_b_channel_binding: usize,

    pub brush_radius_x: f32,
    pub brush_radius_y: f32,
    pub brush_color: ChannelSelection,

    pub eraser_radius_x: f32,
    pub eraser_radius_y: f32,
    pub eraser_color: ChannelSelection,

    pub utilization: Arc<AtomicU32>,
    pub debug: Arc<AtomicI32>,
}

impl Frontend {
    pub fn update_plot(&mut self) {
        self.data.read_with(|data| {
            let buffer = data.construct_rgba_buffer_from_plots(
                self.plot_r_channel_binding,
                self.plot_g_channel_binding,
                self.plot_b_channel_binding,
            );
            self.plot_texture_handle.as_mut().unwrap().set(
                egui::ColorImage::from_rgba_unmultiplied(
                    [data.x_width_plot(), data.frequency_count as usize],
                    &buffer,
                ),
                TextureOptions::NEAREST,
            )
        })
    }

    pub fn apply_brush(&self, pos: (f32, f32), data: &mut Data) {
        data.for_pixel_in_brush_range(
            pos,
            (self.brush_radius_x, self.brush_radius_y),
            &self.brush_color,
            |(_x, _y), value| *value = 255,
        );
    }

    pub fn apply_eraser(&self, pos: (f32, f32), data: &mut Data) {
        data.for_pixel_in_brush_range(
            pos,
            (self.eraser_radius_x, self.eraser_radius_y),
            &self.eraser_color,
            |(_x, _y), value| *value = 0,
        );
    }
}

impl eframe::App for Frontend {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.input(|i| i.viewport().close_requested());
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);

        self.plot_texture_handle.get_or_insert_with(|| {
            self.data.read_with(|data| {
                let buffer = data.construct_rgba_buffer_from_plots(
                    self.plot_r_channel_binding,
                    self.plot_g_channel_binding,
                    self.plot_b_channel_binding,
                );

                ctx.load_texture(
                    "plot texture",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [data.x_width_plot(), data.frequency_count as usize],
                        &buffer,
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
            ui.label(format!("debug: {}", self.debug.load(Ordering::Relaxed)));
            let img_response = ui.image((
                self.plot_texture_handle.as_ref().unwrap().id(),
                ui.available_size(),
            ));

            if mouse_button == MouseButton::Left {
                if let Some(interaction_pos) = mouse_pos {
                    self.data.write_with(|data| {
                        let pos = map_from_rect(
                            (interaction_pos.x, interaction_pos.y),
                            img_response.rect,
                            (data.x_width_plot() as f32, data.frequency_count as f32),
                        );
                        self.apply_brush(pos, data);
                    });

                    self.update_plot();
                }
            }

            if mouse_button == MouseButton::Right {
                if let Some(interaction_pos) = mouse_pos {
                    self.data.write_with(|data| {
                        let pos = map_from_rect(
                            (interaction_pos.x, interaction_pos.y),
                            img_response.rect,
                            (data.x_width_plot() as f32, data.frequency_count as f32),
                        );
                        self.apply_eraser(pos, data);
                    });

                    self.update_plot();
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
