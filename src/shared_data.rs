use core::f32;
use std::{
    ops::{Index, IndexMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc, RwLock,
    },
};

use crate::{aabb::AABB, lv2_impl::Synth};

#[derive(Clone)]
pub struct Region {
    pub bounding_box: AABB,
    pub data: Vec<f32>,
}

pub struct DataWriter {
    inner: RwLock<Data>,
    recompute_cache: Arc<Sender<()>>,
    interrupt: Arc<AtomicBool>,
}

impl DataWriter {
    pub fn new(recompute_cache: Arc<Sender<()>>, interrupt: Arc<AtomicBool>, data: Data) -> Self {
        Self {
            inner: RwLock::new(data),
            recompute_cache,
            interrupt,
        }
    }

    pub fn read_with<T>(&self, mut with: impl FnMut(&Data) -> T) -> T {
        with(&self.inner.read().unwrap())
    }

    pub fn write_with(&self, synth: &Synth, mut with: impl FnMut(&mut Data) -> Vec<usize>) {
        self.interrupt.store(true, Ordering::SeqCst);
        let mut write_lock = self.inner.write().unwrap();
        self.interrupt.store(false, Ordering::SeqCst);
        let update_chunks = with(&mut write_lock);
        drop(write_lock);

        for chunk_i in &update_chunks {
            let Some(chunk) = synth.sum_cache.get(*chunk_i) else {
                continue;
            };
            chunk
                .invalidate
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        self.recompute_cache.send(()).unwrap();
    }
}

pub struct Data {
    pub frequency_divisions_per_semitone: usize,
    pub frequency_count: usize,

    /// Length of amplitude and other plots in samples.
    pub plot_length: usize,

    pub amplitude_plot: Vec<f32>,
    pub modulation_plot: Vec<f32>,
    pub pan_plot: Vec<f32>,
}

impl Index<usize> for Data {
    type Output = Vec<f32>;
    fn index(&self, index: usize) -> &Self::Output {
        match index {
            0 => &self.amplitude_plot,
            1 => &self.modulation_plot,
            2 => &self.pan_plot,
            _ => panic!("indexed plot {index} not available--out of bounds [0,3)"),
        }
    }
}

impl IndexMut<usize> for Data {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        match index {
            0 => &mut self.amplitude_plot,
            1 => &mut self.modulation_plot,
            2 => &mut self.pan_plot,
            _ => panic!("indexed plot {index} not available--out of bounds [0,3)"),
        }
    }
}

impl Data {
    /// Creates a new data struct that has reasonable defaults
    pub fn new(sample_rate: usize) -> Self {
        let frequency_count = 352;

        let plot_length = sample_rate * 2;

        let plot_size = (plot_length * frequency_count) as usize;

        Self {
            frequency_divisions_per_semitone: 4,
            frequency_count,

            plot_length,

            amplitude_plot: vec![0f32; plot_size],
            modulation_plot: vec![0f32; plot_size],
            pan_plot: vec![0f32; plot_size],
        }
    }

    pub fn construct_rgba_buffer_from_plots(
        &self,
        r_channel: usize,
        g_channel: usize,
        b_channel: usize,
        x_resolution: usize,
        buffer: &mut [u8],
        render_selection: AABB,
    ) {
        let ratio = self.plot_length / x_resolution;

        for freq_i in render_selection.top()..render_selection.bottom().min(self.frequency_count) {
            let mut sum_r = 0f32;
            let mut sum_g = 0f32;
            let mut sum_b = 0f32;
            for sample_i in (render_selection.left() as isize - ratio as isize).max(0) as usize
                ..render_selection.right().min(self.plot_length)
            {
                let plot_index = freq_i * self.plot_length + sample_i;
                sum_r += self[r_channel][plot_index];
                sum_g += self[g_channel][plot_index];
                sum_b += self[b_channel][plot_index];

                if sample_i % ratio == 0 && sample_i >= render_selection.left() {
                    buffer[plot_index / ratio * 4] =
                        (sum_r * 255f32 / ratio as f32).min(255f32) as u8;
                    buffer[plot_index / ratio * 4 + 1] =
                        (sum_g * 255f32 / ratio as f32).min(255f32) as u8;
                    buffer[plot_index / ratio * 4 + 2] =
                        (sum_b * 255f32 / ratio as f32).min(255f32) as u8;
                    buffer[plot_index / ratio * 4 + 3] = 255;

                    sum_r = 0f32;
                    sum_g = 0f32;
                    sum_b = 0f32;
                }
            }
        }
    }

    pub fn get_region(&self, selection: AABB, brush_color: &ChannelSelection) -> Region {
        let mut ret = vec![0f32; selection.area()];
        for y in selection.top()..selection.bottom() {
            let region_y = y - selection.top();
            ret[region_y * selection.width()..(region_y + 1) * selection.width()].copy_from_slice(
                &self[brush_color.index()][y * self.plot_length + selection.left()
                    ..y * self.plot_length + selection.right()],
            );
        }
        Region {
            bounding_box: selection,
            data: ret,
        }
    }

    pub fn set_region(&mut self, region: &Region, brush_color: &ChannelSelection) -> AABB {
        for y in region.bounding_box.top()..region.bounding_box.bottom() {
            let self_offset = y * self.plot_length + region.bounding_box.left();
            let region_offset = (y - region.bounding_box.top()) * region.bounding_box.width();
            self[brush_color.index()][self_offset..self_offset + region.bounding_box.width()]
                .copy_from_slice(
                    &region.data[region_offset..region_offset + region.bounding_box.width()],
                );
        }
        region.bounding_box.clone()
    }

    /// (takes coordinates in the space of the plot, not global coordinates)
    pub fn for_pixel_in_brush_range(
        &self,
        brush_pos: (f32, f32),
        brush_radii: (f32, f32),
        brush_color: &ChannelSelection,
        mut for_each: impl FnMut((usize, usize), f32) -> f32,
    ) -> Option<Region> {
        let channel_index = brush_color.index();

        let start_pos = (
            (brush_pos.0 - brush_radii.0).max(0f32) as usize,
            (brush_pos.1 - brush_radii.1).max(0f32) as usize,
        );
        let end_pos = (
            ((brush_pos.0 + brush_radii.0).round() as usize + 1).min(self.plot_length),
            ((brush_pos.1 + brush_radii.1).round() as usize).min(self.frequency_count),
        );

        let Some(bounding_box) = AABB::new(start_pos, end_pos) else {
            return None;
        };
        let mut region = self.get_region(bounding_box, brush_color);

        for y in start_pos.1..end_pos.1 {
            for x in start_pos.0..end_pos.0 {
                region.data[region.bounding_box.width() * (y - start_pos.1) + x - start_pos.0] =
                    for_each((x, y), self[channel_index][y * self.plot_length + x]);
            }
        }

        Some(region)
    }
}

pub enum ChannelSelection {
    Red,
    Green,
    Blue,
}

impl ChannelSelection {
    pub fn index(&self) -> usize {
        match self {
            ChannelSelection::Red => 0,
            ChannelSelection::Green => 1,
            ChannelSelection::Blue => 2,
        }
    }
}
