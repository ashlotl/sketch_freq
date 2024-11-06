use core::f32;
use std::ops::{Index, IndexMut};

pub struct Data {
    pub semitone_divisions: usize,
    pub frequency_count: u32,

    /// (in seconds)
    pub plot_duration: f32,
    pub second_subdivisions: u32,

    pub amplitude_plot: Vec<u8>,
    pub modulation_plot: Vec<u8>,
    pub pan_plot: Vec<u8>,

    pub caches_invalidated: bool,
    pub reinvalidate_cache: bool,
}

impl Index<usize> for Data {
    type Output = Vec<u8>;
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
    pub fn new() -> Self {
        let frequency_count = 352;
        let plot_duration = 2f32;
        let second_subdivisions = 64;

        let x_width = plot_duration * second_subdivisions as f32;
        let plot_size = (x_width as u32 * frequency_count) as usize;

        Self {
            semitone_divisions: 4,
            frequency_count,
            plot_duration,
            second_subdivisions,

            amplitude_plot: vec![0; plot_size],
            modulation_plot: vec![0; plot_size],
            pan_plot: vec![0; plot_size],

            caches_invalidated: true,
            reinvalidate_cache: false,
        }
    }

    pub fn x_width_plot(&self) -> usize {
        (self.plot_duration * self.second_subdivisions as f32) as usize
    }

    pub fn construct_rgba_buffer_from_plots(
        &self,
        r_channel: usize,
        g_channel: usize,
        b_channel: usize,
    ) -> Vec<u8> {
        let x_width = self.x_width_plot();

        let minimum_capacity = x_width * self.frequency_count as usize * 4;
        let mut buffer = Vec::with_capacity(minimum_capacity);

        for i in 0..minimum_capacity {
            match i % 4 {
                0 => buffer.push(self[r_channel][i / 4]),
                1 => buffer.push(self[g_channel][i / 4]),
                2 => buffer.push(self[b_channel][i / 4]),
                3 => buffer.push(255),
                _ => unreachable!(),
            }
        }

        buffer
    }

    /// (takes coordinates in the space of the plot, not global coordinates)
    pub fn for_pixel_in_brush_range(
        &mut self,
        brush_pos: (f32, f32),
        brush_radii: (f32, f32),
        brush_color: &ChannelSelection,
        for_each: impl Fn((usize, usize), &mut u8),
    ) {
        let x_width = self.x_width_plot();

        let channel_index = brush_color.index();

        let start_pos = (
            (brush_pos.0 - brush_radii.0).max(0f32) as usize,
            (brush_pos.1 - brush_radii.1).max(0f32) as usize,
        );
        let end_pos = (
            (brush_pos.0 + brush_radii.0).round().min(x_width as f32) as usize,
            (brush_pos.1 + brush_radii.1)
                .round()
                .min(self.frequency_count as f32) as usize,
        );

        for y in start_pos.1..end_pos.1 {
            for x in start_pos.0..end_pos.0 {
                let x = x;
                let y = y;
                let dx = x as f32 - brush_pos.0;
                let dy = y as f32 - brush_pos.1;

                //check if inside ellipse
                if dx * dx / brush_radii.0 + dy * dy / brush_radii.1 > 1f32 {
                    continue;
                }

                for_each((x, y), &mut self[channel_index][y * x_width + x]);
            }
        }
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
